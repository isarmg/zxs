//! A bounded, append-only receiver for one of an actually reserved first
//! SAVE's two material roles. An existing prefix is compared, never repaired
//! by overwriting. An unmatched name is not enough to recover an original.
use super::*;
use crate::system::reader::smb::first_work::Reserved;
use std::os::unix::fs::FileExt;
unsafe extern "C"{
    fn zxs_create_new_regular(parent:c_int,name:*const c_char)->c_int;
    fn linkat(olddir:c_int,old:*const c_char,newdir:c_int,new:*const c_char,flags:c_int)->c_int;
}
pub(crate) struct WorkPair<'d>{dir:&'d HeldDir,primary:CString,second:CString,file:File,stamp:LiveStamp,at:u64,limit:u64}
impl<'d> WorkPair<'d>{
    pub(crate) fn open(work:&'d Reserved<'_>,table:bool)->io::Result<Self>{
        let (installed,spec)=work.parts();let dir=installed.directory();WitnessRead::require_parent(dir)?;
        let [data,pin_data,checks,pin_checks]=spec.names()?;
        let(primary,second,limit)=if table{(checks,pin_checks,spec.table_len()?)}else{(data,pin_data,spec.bytes)};
        dir.checked_child_path(&primary)?;dir.checked_child_path(&second)?;
        let file=match (dir.pin_record(&primary)?,dir.pin_record(&second)?){
            (None,None)=>{
                let n=unsafe{zxs_create_new_regular(dir.file.as_raw_fd(),primary.as_ptr())};
                if n<0{return Err(io::Error::last_os_error());}let f=unsafe{File::from_raw_fd(n)};
                let a=f.metadata()?;
                if !a.is_file()||a.ino()==0||a.uid()!=local_uid()||a.mode()&0o7777!=0o600||a.nlink()!=1||a.len()!=0||!dir.shares_file_mount(&f)?{
                    return Err(invalid("not a fresh private working material"));}
                let n=unsafe{linkat(dir.file.as_raw_fd(),primary.as_ptr(),dir.file.as_raw_fd(),second.as_ptr(),0)};
                if n<0{return Err(io::Error::last_os_error());}f
            }
            (Some(pin),Some(_))=>{
                let size=LiveStamp::read(&pin)?.len;
                if size>limit{return Err(invalid("pending original exceeds its fixed working extent"));}
                let original=WitnessRead::at(dir)?.open(&primary,&second,size)?;
                // Use only the pinned, already verified private regular pair.
                let n=unsafe{zxs_open_pinned_record(original.file().as_raw_fd(),1)};
                if n<0{return Err(io::Error::last_os_error());}let f=unsafe{File::from_raw_fd(n)};
                original.stamp().require(&f)?;original.unchanged()?;f
            }
            _=>return Err(invalid("unproven partial work name; preserve the site and reservation")),
        };
        let stamp=LiveStamp::read(&file)?;
        let p=Self{dir,primary,second,file,stamp,at:0,limit};p.require()?;Ok(p)
    }
    pub(crate) fn stamp(&self)->LiveStamp{self.stamp}
    fn require(&self)->io::Result<()>{
        WitnessRead::require_parent(self.dir)?;
        let m=self.file.metadata()?;
        if !m.is_file()||m.ino()==0||m.uid()!=local_uid()||m.mode()&0o7777!=0o600||m.nlink()!=2||m.len()>self.limit||
            !self.dir.shares_file_mount(&self.file)?{return Err(invalid("original working file is no longer private and bounded"));}
        for n in [&self.primary,&self.second]{
            let pin=self.dir.pin_record(n)?.ok_or_else(||invalid("a working role name vanished"))?;
            if !self.dir.shares_file_mount(&pin)?{return Err(invalid("working role moved mounts"));}
            self.stamp.require(&pin)?;
        }
        self.stamp.require(&self.file)
    }
    pub(crate) fn put(&mut self,offset:u64,bytes:&[u8])->io::Result<()>{
        let end=offset.checked_add(bytes.len() as u64).filter(|x|*x<=self.limit)
            .ok_or_else(||invalid("working append exceeds original funded length"))?;
        if offset!=self.at||offset>self.stamp.len{return Err(invalid("non-contiguous first working input"));}
        self.require()?;
        let overlap=(end.min(self.stamp.len)-offset) as usize;
        let mut scratch=[0u8;32768];let mut checked=0;
        while checked<overlap{let n=(overlap-checked).min(scratch.len());
            self.file.read_exact_at(&mut scratch[..n],offset+checked as u64)?;
            if scratch[..n]!=bytes[checked..checked+n]{return Err(invalid("unconfirmed material prefix differs; left intact"));}checked+=n;}
        if overlap<bytes.len(){
            self.stamp.require(&self.file)?;
            self.file.write_all_at(&bytes[overlap..],offset+overlap as u64)?;
            let next=LiveStamp::read(&self.file)?;
            if !self.stamp.same(next)||next.len!=end{return Err(invalid("working material changed during append"));}
            self.stamp=next;
        }
        self.at=end;Ok(())
    }
    pub(crate) fn finish(&self)->io::Result<()>{
        if self.at!=self.limit||self.stamp.len!=self.limit{return Err(invalid("incomplete working material"));}
        self.require()?;self.file.sync_all()?;self.require()
    }
}
