//! Read-only Web history: exact confirmed node metadata in the local
//! database. It neither checks disk health nor opens a FILE or its body.
use super::{WebAdmin,Connection};
use super::super::{file_location::{Place,Placed},smb::ordinary_directory::Route,work_record};
use crate::{FileId,NodeId,OperationId,Role,VersionId,VolumeId,fs::invalid};
use std::io;

const PAGE:usize=32;
pub(crate) struct Timeline{
    pub(crate) file:FileId,pub(crate) place:TimelinePlace,
    pub(crate) rows:Vec<Version>,pub(crate) next:Option<i64>,
}
#[derive(Debug,Eq,PartialEq)]
pub(crate) enum TimelinePlace{
    Ordinary{name:String,volume:VolumeId},
    Retained{name:String,volume:VolumeId},
    // An exact original pending rename was verified for a selected
    // ancestor. No old spelling, destination or volume leaves this cut.
    ParentPending,
}
pub(crate) struct Version{
    pub(crate) order:i64,pub(crate) node:NodeId,pub(crate) version:VersionId,
    pub(crate) parent:Option<NodeId>,pub(crate) revision:u64,pub(crate) length:u64,
}
fn original_parent(db:&Connection,file:FileId,parent:OperationId,order:i64)->io::Result<NodeId>{
    let q=db.prepare(c"SELECT n.node_id FROM main.manual_node n JOIN main.operation o ON o.op_id=n.commit_op
        WHERE n.file_id=?1 AND n.ord=?2 AND n.commit_op=?3 AND o.kind=11 AND o.state=1")?;
    q.bind_bytes(1,&file.0)?;q.bind_integer(2,order)?;q.bind_bytes(3,&parent.0)?;
    if !work_record::start_one(&q)?{return Err(invalid("missing confirmed parent of manual node"));}
    let n=NodeId(q.array(0)?);q.done()?;
    if n.0==[0;16]{return Err(invalid("malformed original parent node"));}
    Ok(n)
}
impl WebAdmin<'_>{
    /// The cursor is only an inclusive upper ordinal for this exact FILE.
    /// Fixed indexed keyset pagination does not rescan preceding versions.
    pub(crate) fn manual_timeline(&self,file:FileId,upper:Option<i64>)->io::Result<Timeline>{
        let upper=upper.unwrap_or(i64::MAX);
        if upper<1{return Err(invalid("invalid manual timeline cursor"));}
        self.source.at(|db,c|{
            self.current(db,&c)?;
            let Placed{birth,here:at}=Place::current(db,&c,file)?.bind(db,&c)?;
            let place=match at.role{
                Role::File=>match Route::settled_at(db,&c,at.volume,at.parent)?{
                    Some(route)=>TimelinePlace::Ordinary{name:route.show_child(&at.name),volume:at.volume},
                    None=>TimelinePlace::ParentPending,
                },
                Role::Recycle=>TimelinePlace::Retained{name:at.name,volume:at.volume},
                _=>return Err(invalid("non-FILE role in confirmed history")),
            };
            let q=db.prepare(c"SELECT n.ord,n.node_id,n.version_id,n.parent_op,n.revision,n.round_op,n.save_op
             FROM main.manual_node n JOIN main.operation o ON o.op_id=n.commit_op AND o.kind=11 AND o.state=1
             WHERE n.file_id=?1 AND n.ord<=?2 ORDER BY n.ord DESC LIMIT 33")?;
            q.bind_bytes(1,&file.0)?;q.bind_integer(2,upper)?;
            let mut rows=Vec::new();let mut previous=None;
            while work_record::start_one(&q)?{
                let ord=q.integer(0)?;
                if ord<1||ord>upper||previous.is_some_and(|p:i64|p.checked_sub(1)!=Some(ord))||rows.len()>PAGE{
                    return Err(invalid("not consecutive confirmed manual node records"));
                }
                let node=NodeId(q.array(1)?);let version=VersionId(q.array(2)?);
                if node.0==[0;16]||version.0==[0;16]{return Err(invalid("malformed original version identity"));}
                let parent=match work_record::optional(&q,3)?{
                    Some(p) if ord>1=>Some(original_parent(db,file,p,ord-1)?),
                    None if ord==1=>None,
                    _=>return Err(invalid("manual node does not link its previous version")),
                };
                let revision=work_record::natural(&q,4)?;
                let round=OperationId(q.array(5)?);let save=OperationId(q.array(6)?);
                let issued=work_record::confirmed(db,&birth,save)?.record;
                if issued.key().round!=round||issued.key().revision!=revision{
                    return Err(invalid("manual version disagrees with its original confirmed working cut"));
                }
                rows.push(Version{order:ord,node,version,parent,revision,length:issued.logical()});previous=Some(ord);
            }
            let next=if rows.len()>PAGE{rows.pop().map(|v|v.order)}else{None};
            Ok(Timeline{file,place,rows,next})
        })
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    use crate::{system::reader::{InitialReader,ordinary_pool,smb::SmbSession},test_support::PrivateTemp};
    use std::os::unix::fs::{FileExt,OpenOptionsExt};

    #[test]
    #[ignore="true private SQL and multiple FILE work rounds; source only"]
    fn confirmed_history_pages_keep_the_first_unreturned_ordinal_and_original_links(){
        let temp=PrivateTemp::under(&std::env::temp_dir());let (s,p,path,rw,_)=ordinary_pool(&temp);
        let src=InitialReader::open(&s,p).unwrap();let read=InitialReader::open(&s,p).unwrap();
        let account=SmbSession::fixture_authenticated(&src,"files.smb").unwrap();
        let tree=account.connect(rw).unwrap();
        let admin=read.authenticate_web("first.admin","correct horse battery staple").unwrap();
        let file=tree.create_file(OperationId([0x7f;16]),"Tracked.txt").unwrap().file;
        let input=std::fs::OpenOptions::new().write(true).read(true).create_new(true).mode(0o600).open(temp.0.join("input")).unwrap();
        let mut prev=None;let mut originals=Vec::new();
        for i in 0..34u8{
            // The pool already issued repeated-byte IDs for shares/modes.
            // Give both operations in each round an independent test key.
            let id=|kind|{let mut bytes=[0x7f;16];bytes[14]=i;bytes[15]=kind;OperationId(bytes)};
            let op=id(1);let commit=id(2);
            input.write_all_at(&[i],0).unwrap();input.sync_all().unwrap();
            tree.open_writer("Tracked.txt",file).unwrap().save(op,prev,Some(0),1,&input).unwrap();
            originals.push(admin.freeze(commit,admin.manual_target(file).unwrap()).unwrap());prev=Some(op);
        }
        let page=admin.manual_timeline(file,None).unwrap();
        assert_eq!(page.rows.len(),32);assert_eq!(page.next,Some(2));
        assert_eq!(page.rows[0].version,originals[33].version);
        assert_eq!(page.rows[0].parent,Some(originals[32].node));
        // Metadata remains precise even when the actual F is absent.
        let aside=temp.0.join("old-F");std::fs::rename(path,&aside).unwrap();
        let early=admin.manual_timeline(file,page.next).unwrap();
        assert_eq!(early.rows.iter().map(|r|r.order).collect::<Vec<_>>(),vec![2,1]);
        assert_eq!(early.rows[0].length,1);assert_eq!(early.rows[1].node,originals[0].node);
        assert!(early.rows[1].parent.is_none()&&early.next.is_none());
    }
}
