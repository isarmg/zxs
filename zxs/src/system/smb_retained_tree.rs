//! Storage-side read-only routing within the one native ZXS tree. Separate
//! private folder handles do not create shares. No SQL, path or disk FD
//! crosses to Samba; a retained FILE uses the same native stable file key.
use super::{mover,InitialReader,Site,SmbSession,TreeOrigin,denied,live,os,zxs_dir_take,zxs_dir_reply,zxs_member_reply,
    zxs_read_take,zxs_read_fill,zxs_read_accept,zxs_smb_tree_reply,zxs_tree_events,zxs_member_held};
use super::super::super::retained_read::KeepReader;
use crate::OperationId;
use std::{io,os::fd::{AsRawFd,FromRawFd,OwnedFd},sync::atomic::{AtomicBool,AtomicUsize,Ordering},thread};

const R_MEMBER:u8=245;
const R_SHARED_DELETE:u8=246;
const R_FOLDER:u8=253;
const MEMBERS:usize=16;
const PAGE:usize=16;
const SLOT:usize=272; // 1 + 255 + 8 + 8, same native directory envelope
static ROUNDS:AtomicUsize=AtomicUsize::new(0);
struct Slot;
impl Slot{
    fn take()->Option<Self>{
        ROUNDS.fetch_update(Ordering::AcqRel,Ordering::Acquire,|n|(n<128).then_some(n+1)).ok()?;
        Some(Self)
    }
}
impl Drop for Slot{fn drop(&mut self){ROUNDS.fetch_sub(1,Ordering::AcqRel);}}

unsafe extern "C"{fn zxs_r_round_rows(fd:i32,body:*const u8)->i32;}
fn delete_id(raw:&[u8])->io::Result<OperationId>{
    if raw.len()!=32||!raw.iter().all(|&b|matches!(b,b'0'..=b'9'|b'a'..=b'f')){return Err(denied());}
    let op=std::str::from_utf8(raw).map_err(|_|denied())?.parse::<OperationId>().map_err(|_|denied())?;
    if op.0==[0;16]||op.0==[0xff;16]{return Err(denied());}Ok(op)
}
fn member(raw:&[u8])->io::Result<(OperationId,&str)>{
    let (del,tail)=raw.split_at_checked(32).ok_or_else(denied)?;
    let name=tail.strip_prefix(b"\\").ok_or_else(denied)?;
    Ok((delete_id(del)?,crate::display::component(name,222)?))
}
fn row(out:&mut[u8],name:&[u8],number:u64,size:u64){
    out[0]=name.len() as u8;out[1..1+name.len()].copy_from_slice(name);
    out[256..264].copy_from_slice(&number.to_be_bytes());out[264..272].copy_from_slice(&size.to_be_bytes());
}

pub(super) fn serve(site:Site<'_>,origin:TreeOrigin,reply:OwnedFd)->io::Result<()>{
    channel(site,origin,reply,None)
}
fn channel(site:Site<'_>,origin:TreeOrigin,reply:OwnedFd,round:Option<OperationId>)->io::Result<()>{
    if origin.slot.pool!=site.pool||(site.stopping)()||!live(&origin,&reply,0){return Err(denied());}
    site.validate()?;
    let src=InitialReader::alongside(site.dir,site.mark,site.pool,site.shape)?;
    let session=SmbSession{source:&src,original:origin.slot.original.clone()};
    let drive=session.retained()?;
    if let Some(id)=round{drive.deletion(id)?;}
    let mut cut=[0;74];
    cut[16..32].copy_from_slice(&drive.volume().0);
    if let Some(id)=round{cut[32..48].copy_from_slice(&id.0);}
    cut[55]=1;cut[56]=1;cut[57..73].copy_from_slice(&site.pool.0);cut[73]=1;
    if (site.stopping)()||!live(&origin,&reply,0){return Err(denied());}
    site.validate()?;os(unsafe{zxs_smb_tree_reply(reply.as_raw_fd(),cut.as_ptr())})?;
    let serving=AtomicBool::new(true);
    struct Close<'a>(&'a AtomicBool);
    impl Drop for Close<'_>{fn drop(&mut self){self.0.store(false,Ordering::Release);}}
    let stopped=||!serving.load(Ordering::Acquire)||(site.stopping)()||!live(&origin,&reply,0);
    let child=Site{stopping:&stopped,..site};
    thread::scope(|scope|{
        let _stop=Close(&serving);
        let mut children=Vec::new();let mut moves=Vec::new();
        let mut files=Vec::<(OwnedFd,super::super::super::retained_read::Reader<'_,'_,'_>)>::new();
        while !stopped(){
            let mut fds=[-1;MEMBERS];let mut flags=[0;MEMBERS+1];
            for(i,(fd,_))in files.iter().enumerate(){fds[i]=fd.as_raw_fd();}
            if unsafe{zxs_tree_events(reply.as_raw_fd(),origin.session.as_raw_fd(),fds.as_ptr(),files.len(),flags.as_mut_ptr())}<0{break;}
            for i in (0..files.len()).rev(){
                if stopped(){return Err(denied());}
                match flags[i+1]{0=>continue,2=>{files.remove(i);continue;},1=>{},_=>return Err(denied())}
                let fd=files[i].0.as_raw_fd();let action=mover::pending(fd);
                if action<0{files.remove(i);continue;}
                if action==1{
                    let Ok((request,answer))=mover::take(fd,&origin)else{files.remove(i);continue;};
                    if !matches!(&request,mover::Request::Restore{..}){mover::refuse(answer,5);continue;}
                    moves.retain(|job:&thread::ScopedJoinHandle<'_,()>|!job.is_finished());
                    let Some(permit)=mover::Permit::take()else{mover::refuse(answer,1);continue;};
                    let source=files[i].1.place().clone();let worker=origin.duplicate()?;let branch=&reply;
                    if let Ok(job)=thread::Builder::new().name("zxs-restore-to-ordinary".into()).spawn_scoped(scope,move||{
                        mover::run(child,&worker,branch,answer,request,source,None,permit);
                    }){moves.push(job);}
                    continue;
                }
                let(mut at,mut count)=(0,0);
                let ans=unsafe{zxs_read_take(fd,origin.worker.as_raw_fd(),&mut at,&mut count)};
                if ans<0{files.remove(i);continue;}
                let ans=unsafe{OwnedFd::from_raw_fd(ans)};
                if site.validate().is_err()||unsafe{zxs_member_held(fd)}!=0{continue;}
                let ok=files[i].1.smb_read_window(at,count,|data|
                    os(unsafe{zxs_read_fill(ans.as_raw_fd(),data.as_ptr(),data.len())}));
                if ok.is_ok()&&!stopped()&&site.validate().is_ok()&&unsafe{zxs_member_held(fd)}==0{
                    let _=unsafe{zxs_read_accept(ans.as_raw_fd())};
                }
            }
            if flags[0]==0{continue;}
            let mut action=0;let mut label=[0;256];
            let fd=unsafe{zxs_dir_take(reply.as_raw_fd(),origin.worker.as_raw_fd(),&mut action,label.as_mut_ptr())};
            if fd<0{break;}
            let answer=unsafe{OwnedFd::from_raw_fd(fd)};
            let input=&label[1..1+usize::from(label[0])];
            if action==R_FOLDER&&round.is_none(){
                let Ok(del)=delete_id(input)else{continue;};
                children.retain(|h:&thread::ScopedJoinHandle<'_,()>|!h.is_finished());
                if children.len()>=MEMBERS{continue;}
                let Some(permit)=Slot::take()else{continue;};
                let peer=origin.duplicate()?;
                if let Ok(handle)=thread::Builder::new().name("zxs-recycle-round".into()).spawn_scoped(scope,move||{
                    let _permit=permit;
                    let _=channel(child,peer,answer,Some(del));
                }){children.push(handle);}
            }else if matches!(action,R_MEMBER|R_SHARED_DELETE)&&round.is_none(){
                if files.len()>=MEMBERS{continue;}
                let Ok((op,name))=member(input)else{continue;};
                let keep=if action==R_SHARED_DELETE{KeepReader::ShareDelete}else{KeepReader::WhileRetained};
                let Ok((held,key))=drive.open(op,name,keep)else{continue;};
                let ids=held.identifiers();
                let mut data=[0;88];
                for (i,one) in ids.iter().enumerate(){data[i*16..i*16+16].copy_from_slice(one);}
                for (i,one) in [key.words().1,held.revision(),held.size()].iter().enumerate(){
                    data[64+i*8..72+i*8].copy_from_slice(&one.to_be_bytes());
                }
                if stopped()||site.validate().is_err(){break;}
                if unsafe{zxs_member_reply(answer.as_raw_fd(),data.as_ptr())}==0{files.push((answer,held));}
            }else if usize::from(action)<=PAGE{
                let data=(||->io::Result<[u8;1+PAGE*SLOT]>{
                    let mut packet=[0;1+PAGE*SLOT];
                    if let Some(id)=round{
                        let item=drive.deletion(id)?;
                        if !input.is_empty(){crate::display::component(input,222)?;}
                        if action!=0&&item.name.as_bytes()>input{
                            let (held,key)=drive.open(id,&item.name,KeepReader::WhileRetained)?;
                            packet[0]=1;
                            row(&mut packet[1..1+SLOT],item.name.as_bytes(),key.words().1,held.size());
                        }
                    }else{
                        let after=if input.is_empty(){None}else{Some(delete_id(input)?)};
                        let page=drive.page(after,action)?;packet[0]=page.len() as u8;
                        for (item,w) in page.iter().zip(packet[1..].chunks_exact_mut(SLOT)){
                            row(w,item.op.to_string().as_bytes(),u64::MAX,0);
                        }
                    }
                    Ok(packet)
                })();
                if let Ok(data)=data{
                    if stopped()||site.validate().is_err(){break;}
                    if round.is_none(){let _=unsafe{zxs_r_round_rows(answer.as_raw_fd(),data.as_ptr())};}
                    else{let _=unsafe{zxs_dir_reply(answer.as_raw_fd(),data.as_ptr())};}
                }
            }
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn an_r_member_names_one_exact_lowercase_delete_round_and_original_component(){
        let good=b"1234567890abcdef0000000000000001\\Report.txt";
        assert_eq!(member(good).unwrap().1,"Report.txt");
        assert!(member(b"1234567890ABCDEF0000000000000001\\Report.txt").is_err());
        assert!(member(b"00000000000000000000000000000000\\Report.txt").is_err());
        assert!(member(b"1234567890abcdef0000000000000001\\nested\\Report.txt").is_err());
    }
}
