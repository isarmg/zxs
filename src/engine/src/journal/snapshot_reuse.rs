//! Full exact dependency reads and immutable barriers remain until the scrub gate.
use super::*;
impl SnapshotReuse {
    pub(crate) fn matches_snapshot(&self, hash: &str) -> bool {
        self.hash == hash
    }
}
impl PreparedSnapshot {
    /// Every entry here came from this encoder invocation, even if its digest
    /// names an older object. Installation therefore always compares and syncs.
    #[cfg(test)]
    pub(crate) fn store_new_objects(&self, members: &[DiskConfig]) -> Result<String> {
        self.store_created(|object| object.store(members))
    }
    pub(crate) fn store_new_objects_at(
        &self,
        resources: &DiskResources,
        members: &[DiskConfig],
        installs: &mut ObjectInstalls,
    ) -> Result<String> {
        self.store_created(|object| {
            for disk in members {
                object.store_on(&resources.get(disk)?.root, disk)?;
                installs.record(object.bytes.len());
            }
            Ok(object.hash.clone())
        })
    }
    fn store_created(
        &self,
        mut install: impl FnMut(&PreparedMetadataObject) -> Result<String>,
    ) -> Result<String> {
        self.root();
        for object in &self.objects {
            install(object)?;
        }
        Ok(self.hash.clone())
    }
    pub(crate) fn reuse_basis(&self) -> SnapshotReuse {
        self.root();
        SnapshotReuse {
            descriptor: self.descriptor.clone(),
            hash: self.hash.clone(),
            fields: self.fields.clone(),
            auth_bytes: self.auth_bytes,
        }
    }
    pub(crate) fn content_state_budget(&self) -> Option<usize> {
        self.fields["content_states"].maximum_tombstone_tail()
    }
    fn root(&self) -> &PreparedMetadataObject {
        let root = self
            .objects
            .last()
            .expect("invariant: prepared snapshot ends with its root");
        assert_eq!(
            root.kind, "snapshot",
            "invariant: prepared snapshot ends with its root"
        );
        assert_eq!(
            root.hash, self.hash,
            "invariant: prepared snapshot root hash matches"
        );
        root
    }
    #[cfg(test)]
    pub(crate) fn store(&self, members: &[DiskConfig], state: &State) -> Result<String> {
        self.store_with(members, state, |disk, kind, hash, bytes| {
            write_object_bytes(disk, kind, hash, bytes)
        })
    }
    pub(crate) fn store_at(
        &self,
        resources: &DiskResources,
        members: &[DiskConfig],
        state: &State,
        installs: &mut ObjectInstalls,
    ) -> Result<String> {
        self.store_with_read(
            members,
            state,
            |disk, kind, hash, maximum| {
                object_storage::read_object_bytes_bounded_at(
                    &resources.get(disk)?.root,
                    disk,
                    kind,
                    hash,
                    maximum,
                )
            },
            |disk, kind, hash, bytes| {
                write_object_bytes_at(&resources.get(disk)?.root, disk, kind, hash, bytes)?;
                installs.record(bytes.len());
                Ok(())
            },
        )
    }
    #[cfg(test)]
    fn store_with(
        &self,
        members: &[DiskConfig],
        state: &State,
        install: impl FnMut(&DiskConfig, &str, &str, &[u8]) -> Result<()>,
    ) -> Result<String> {
        self.store_with_read(
            members,
            state,
            object_storage::read_object_bytes_bounded,
            install,
        )
    }
    fn store_with_read(
        &self,
        members: &[DiskConfig],
        state: &State,
        mut read: impl FnMut(&DiskConfig, &str, &str, usize) -> Result<Vec<u8>>,
        mut install: impl FnMut(&DiskConfig, &str, &str, &[u8]) -> Result<()>,
    ) -> Result<String> {
        let root = self.root();
        assert_eq!(
            self.fields.len(),
            self.descriptor.maps.len() + 1,
            "invariant: every snapshot field has an installation plan"
        );
        let prepared = self
            .objects
            .iter()
            .map(|object| ((object.kind, object.hash.as_str()), object))
            .collect::<BTreeMap<_, _>>();
        // This bounded, invocation-local set records completed barriers only;
        // exhaustion repeats work rather than dropping verification.
        let mut installed = BTreeSet::new();
        let mut fault = |error: &Error| {
            if let Some(integrity) = &self.metadata_integrity {
                integrity.fault(error);
            }
        };
        for (field, root) in &self.fields {
            let mut rebuilt = None;
            root.visit(&mut |reference, maximum| {
                let kind = reference.kind().domain();
                let hash = reference.hash();
                let address = (kind, hash.to_owned());
                if installed.contains(&address) {
                    return Ok(());
                }
                if let Some(object) = prepared.get(&(kind, hash)) {
                    for disk in members {
                        install(disk, object.kind, &object.hash, &object.bytes)?;
                    }
                    if installed.len() < 16_384 {
                        installed.insert(address);
                    }
                    return Ok(());
                }
                let bytes = match read_published_object_with(
                    members,
                    kind,
                    hash,
                    maximum.min(self.maximum),
                    &mut fault,
                    &mut read,
                ) {
                    Ok(bytes) => bytes,
                    Err(_) => {
                        if rebuilt.is_none() {
                            let (candidate, objects) =
                                snapshot::reconstruct_field(state, field, self.maximum)?;
                            if candidate.reference() != root.reference() {
                                return Err(invalid_reconstruction());
                            }
                            rebuilt = Some(
                                objects
                                    .into_iter()
                                    .map(|object| {
                                        ((object.kind, object.hash.clone()), object.bytes)
                                    })
                                    .collect::<BTreeMap<_, _>>(),
                            );
                        }
                        rebuilt
                            .as_ref()
                            .expect("invariant: failed source reconstructed once")
                            .get(&(kind, hash.to_owned()))
                            .expect("invariant: exact reconstructed root covers every dependency")
                            .clone()
                    }
                };
                for disk in members {
                    install(disk, kind, hash, &bytes)?;
                }
                if installed.len() < 16_384 {
                    installed.insert(address);
                }
                Ok(())
            })?;
        }
        // Sealed and open segment bodies hang off directory leaf values, not
        // radix references, so the field visit above does not reach them.
        // New bodies (open tail, a seal decided this transaction) install
        // from this invocation's encoder output; reused sealed bodies get
        // the same full read-back, byte verification and member sync as
        // every other reused dependency, with the same exact-field
        // reconstruction fallback on source loss.
        let mut segment_dependency = |field: &'static str,
                                      kind: &'static str,
                                      hash: &str,
                                      installed: &mut BTreeSet<(&'static str, String)>|
         -> Result<()> {
            let address = (kind, hash.to_owned());
            if installed.contains(&address) {
                return Ok(());
            }
            if let Some(object) = prepared.get(&(kind, hash)) {
                for disk in members {
                    install(disk, object.kind, &object.hash, &object.bytes)?;
                }
            } else {
                let bytes = match read_published_object_with(
                    members,
                    kind,
                    hash,
                    self.maximum,
                    &mut fault,
                    &mut read,
                ) {
                    Ok(bytes) => bytes,
                    Err(source) => {
                        let (_, objects) = snapshot::reconstruct_field(state, field, self.maximum)?;
                        objects
                            .into_iter()
                            .find(|object| object.kind == kind && object.hash == hash)
                            .ok_or_else(|| {
                                source.context(
                                    ErrorCode::ObjectCorrupt,
                                    "sealed segment has no readable or resident source",
                                )
                            })?
                            .bytes
                    }
                };
                for disk in members {
                    install(disk, kind, hash, &bytes)?;
                }
            }
            if installed.len() < 16_384 {
                installed.insert(address);
            }
            Ok(())
        };
        for reference in state.events.sealed() {
            segment_dependency(
                "events",
                storage_contract::EVENT_SEGMENT_KIND,
                &reference.hash,
                &mut installed,
            )?;
        }
        for reference in state.operations.sealed() {
            segment_dependency(
                "operation_log",
                storage_contract::OPERATION_SEGMENT_KIND,
                &reference.hash,
                &mut installed,
            )?;
        }
        // Commit values reference immutable namespace graphs that the radix
        // visit does not reach. New objects install from this invocation's
        // encoder output; reused graphs get the same full read-back, byte
        // verification and member sync as every other reused dependency. A
        // namespace object lost on every member is history loss and fails
        // closed: it is not reconstructible from resident state.
        // With no members there is nothing to install or synchronize: the
        // store is a pure re-encode (test fixtures derive hashes that way),
        // so the closure sync below would have no source and no destination.
        let mut namespace_visited = BTreeSet::new();
        for commit in state.commits.values().filter(|_| !members.is_empty()) {
            crate::namespace::visit_namespace_objects(
                &commit.namespace.root,
                &mut |kind, hash| {
                    let address = (kind, hash.to_owned());
                    let bytes = if let Some(object) = prepared.get(&(kind, hash)) {
                        for disk in members {
                            install(disk, object.kind, &object.hash, &object.bytes)?;
                        }
                        object.bytes.clone()
                    } else {
                        let bytes = read_published_object_with(
                            members,
                            kind,
                            hash,
                            self.maximum,
                            &mut fault,
                            &mut read,
                        )?;
                        for disk in members {
                            install(disk, kind, hash, &bytes)?;
                        }
                        bytes
                    };
                    if installed.len() < 16_384 {
                        installed.insert(address);
                    }
                    AuthenticatedMetadataObject::authenticate(
                        kind,
                        hash,
                        json_codec::from_slice(&bytes)?,
                    )
                },
                &mut namespace_visited,
            )?;
        }
        for object in &self.objects {
            if matches!(
                object.kind,
                storage_contract::EVENT_SEGMENT_KIND
                    | storage_contract::OPERATION_SEGMENT_KIND
                    | storage_contract::NAMESPACE_PAGE_KIND
                    | storage_contract::NAMESPACE_INDEX_KIND
            ) && !installed.contains(&(object.kind, object.hash.clone()))
            {
                for disk in members {
                    install(disk, object.kind, &object.hash, &object.bytes)?;
                }
                if installed.len() < 16_384 {
                    installed.insert((object.kind, object.hash.clone()));
                }
            }
        }
        let hash = self.descriptor.control_auth.hash();
        if let Some(object) = prepared.get(&("state-control-auth", hash)) {
            for disk in members {
                install(disk, object.kind, &object.hash, &object.bytes)?;
            }
        } else {
            let bytes = match read_published_object_with(
                members,
                "state-control-auth",
                hash,
                self.auth_bytes.min(self.maximum),
                &mut fault,
                &mut read,
            ) {
                Ok(bytes) => bytes,
                Err(_) => {
                    let object = PreparedMetadataObject::new(
                        "state-control-auth",
                        &state.control_auth,
                        self.maximum,
                    )?;
                    if object.hash != hash {
                        return Err(invalid_reconstruction());
                    }
                    object.bytes
                }
            };
            for disk in members {
                install(disk, "state-control-auth", hash, &bytes)?;
            }
        }
        for disk in members {
            install(disk, root.kind, &root.hash, &root.bytes)?;
        }
        Ok(self.hash.clone())
    }
}
fn invalid_reconstruction() -> Error {
    Error::new(
        ErrorCode::ObjectCorrupt,
        "reconstructed snapshot graph differs from its exact bound root",
    )
}
#[cfg(test)]
fn read_published_object(
    members: &[DiskConfig],
    kind: &str,
    hash: &str,
    maximum: usize,
    fault: &mut impl FnMut(&Error),
) -> Result<Vec<u8>> {
    read_published_object_with(
        members,
        kind,
        hash,
        maximum,
        fault,
        &mut object_storage::read_object_bytes_bounded,
    )
}
fn read_published_object_with(
    members: &[DiskConfig],
    kind: &str,
    hash: &str,
    maximum: usize,
    fault: &mut impl FnMut(&Error),
    read: &mut impl FnMut(&DiskConfig, &str, &str, usize) -> Result<Vec<u8>>,
) -> Result<Vec<u8>> {
    let mut first = None;
    for disk in members {
        let result = (|| {
            let bytes = read(disk, kind, hash, maximum)?;
            #[cfg(test)]
            REUSED_OBJECT_PEAK_BYTES.with(|peak| peak.set(peak.get().max(bytes.len())));
            // Provenance is exact current-process encoder output, not a generic
            // on-disk semantic hash. Raw equality proves those canonical bytes.
            let mut digest = blake3::Hasher::new();
            digest.update(b"zxs:");
            digest.update(kind.as_bytes());
            digest.update(b"\0");
            digest.update(&bytes);
            if digest.finalize().to_hex().as_str() != hash {
                return Err(Error::new(
                    ErrorCode::ObjectCorrupt,
                    "published metadata bytes differ",
                ));
            }
            Ok(bytes)
        })();
        match result {
            Ok(bytes) => return Ok(bytes),
            Err(error) => {
                fault(&error);
                first.get_or_insert(error);
            }
        }
    }
    Err(first.unwrap_or_else(|| {
        Error::new(ErrorCode::ObjectCorrupt, "published metadata has no source")
    }))
}

#[cfg(test)]
thread_local! { static REUSED_OBJECT_PEAK_BYTES:std::cell::Cell<usize>=const{std::cell::Cell::new(0)}; }
#[cfg(test)]
#[path = "../../../../test/engine/unit/journal/snapshot_reuse_tests.rs"]
mod tests;
