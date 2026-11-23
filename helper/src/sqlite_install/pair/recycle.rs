//! Retained fixture for the old separate backing observations; never selects a disk.
use crate::pinned::{Entity, Mount};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Identity {
    entity: Entity,
    mount: Mount,
    backing: u64,
}
impl Identity {
    fn separate(self, other: Self) -> bool {
        self.entity.dev != other.entity.dev
            && self.mount.id != other.mount.id
            && self.backing != other.backing
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn key(dev: u64, mount: u64, backing: u64) -> Identity {
        Identity {
            entity: Entity { dev, ino: 4 },
            mount: Mount {
                id: mount,
                at_root: Some(true),
            },
            backing,
        }
    }
    #[test]
    fn all_three_backing_observations_must_be_separate() {
        let a = key(1, 2, 3);
        assert!(a.separate(key(4, 5, 6)));
        for reused in [key(1, 5, 6), key(4, 2, 6), key(4, 5, 3)] {
            assert!(!a.separate(reused));
        }
    }
}
