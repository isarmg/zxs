//! Optional complete ZXS image. There is no default or substitute asset.
use super::{MAX_IMAGE,denied,standalone};
use sha2::{Digest,Sha256};
use std::io;

fn verify(bytes:&[u8],compiler_expected:[u8;32],approved:[u8;32])->io::Result<[u8;32]>{
    if compiler_expected==[0;32]||approved!=compiler_expected||bytes.is_empty()||bytes.len() as u64>MAX_IMAGE||
        <[u8;32]>::from(Sha256::digest(bytes))!=approved{
        return Err(denied());
    }
    standalone(bytes.len() as u64,|offset,out|{
        let start=usize::try_from(offset).map_err(|_|denied())?;
        let end=start.checked_add(out.len()).ok_or_else(denied)?;
        out.copy_from_slice(bytes.get(start..end).ok_or_else(denied)?);Ok(())
    })?;
    Ok(approved)
}

// Type-check the full protected publisher in unit tests without supplying
// any asset or making the command available in a normal binary.
#[cfg(any(test,feature="embedded-zxs"))]
#[cfg_attr(not(feature="embedded-zxs"),allow(dead_code))]
mod enabled{
    use super::{denied,verify};
    use crate::{pinned::{Entity,Mount},sqlite_install::{approval,pair::{
        command::{sha,value},domain::FreshParent,fresh_issuer::{require_private_install,hex},
        terminal::PublicConfirmation},product_layout,stage::random}};
    use crate::sqlite_install::pair::fresh_issuer::image::{Image,path::{OriginalPath,PIN,WITHIN,no_attribute}};
    #[cfg(feature="embedded-zxs")]
    use super::MAX_IMAGE;
    use helper_linux_evidence::diagnostic_process::{cancelled,watch_cancellation};
    use rustix::fs::{AtFlags,Mode,OFlags,StatVfsMountFlags,fchmod,fstatvfs,linkat,openat2};
    use std::{ffi::OsString,fs::File,io,os::unix::fs::{FileExt,MetadataExt},path::PathBuf,time::{Duration,Instant}};

    // Set only while building the optional raw Helper, not read from the running
    // environment. A normal Cargo build has neither dependency nor fake payload.
    #[cfg(feature="embedded-zxs")]
    static ASSET:&[u8]=include_bytes!(env!("HELPER_ZXS_PAYLOAD_FILE","the optional Helper needs one approved complete release ELF"));
    #[cfg(feature="embedded-zxs")]
    const HASH:&str=env!("HELPER_ZXS_PAYLOAD_SHA256","the optional Helper needs the independently approved release SHA-256");
    #[cfg(feature="embedded-zxs")]
    const _:()=assert!(ASSET.len()>=64&&(ASSET.len() as u64)<=MAX_IMAGE);
    const DEST:&std::ffi::CStr=c"zxs";

    #[derive(Debug)]
    pub(crate) struct EmbeddedInstall{directory:PathBuf,independent:[u8;32]}
    impl EmbeddedInstall{
        pub(crate) fn parse(args:impl Iterator<Item=OsString>)->Result<Self,&'static str>{
            let mut args=args.peekable();
            let directory=if args.peek().is_some_and(|s|s=="--destination-dir"){
                PathBuf::from(value(&mut args,"--destination-dir")?)
            }else{PathBuf::from(product_layout::BIN)};
            super::super::path::check_parent(&directory).map_err(|_|"select an existing protected canonical absolute destination directory")?;
            let independent=sha(&mut args,"--zxs-sha256")?;
            if args.next().is_some(){return Err("only this fixed destination and independent digest; no replace, startup or alternate command");}
            Ok(Self{directory,independent})
        }
        #[cfg(test)]
        pub(super) fn selected_directory(&self)->&std::path::Path{&self.directory}
        #[cfg(feature="embedded-zxs")]
        pub(crate) fn run(self)->io::Result<()>{
            let packaged=crate::fs_tools::hex(HASH).ok_or_else(denied)?;
            self.publish(ASSET,packaged)
        }
        // The production caller can only pass the compiled immutable asset
        // and digest. Tests compile this path but never enter it.
        fn publish(self,asset:&[u8],packaged:[u8;32])->io::Result<()>{
            fn absent(dir:&File)->io::Result<()>{
                match openat2(dir,DEST,PIN,Mode::empty(),WITHIN){
                    Err(rustix::io::Errno::NOENT)=>Ok(()),
                    Ok(_)=>Err(io::Error::new(io::ErrorKind::AlreadyExists,"the fixed ZXS executable name is already occupied; left untouched")),
                    Err(e)=>Err(e.into()),
                }
            }
            require_private_install()?;
            let digest=verify(asset,packaged,self.independent)?;
            approval::no_inherited_descriptors()?;
            // The same dedicated real ZXS product lock as issuance. Source
            // refuses any prior site/partial state rather than taking it over.
            let scope=random::<16>()?;
            let site=FreshParent::select(scope)?;
            let tty=PublicConfirmation::select()?; // do not keep extra FDs before the exact root entry gate
            let product=site.product_root()?;
            let target=OriginalPath::selected(&self.directory,true,product)?;
            let directory=target.directory()?;
            let identity=|(f,m):(&File,u64)|->io::Result<_>{
                if Mount::on(f)?.is_none_or(|got|got.id!=m){return Err(denied());}
                Entity::of(&f.metadata()?)
            };
            let mount=Mount::on(&directory)?.ok_or_else(denied)?.id;
            let dir_id=identity((&directory,mount))?;
            let path=target.path().join("zxs");
            super::super::path::check_image(&path)?;
            let protected=site.protect_image_destination(dir_id)?;
            if target.includes(protected){return Err(denied());}
            let require=||->io::Result<()>{
                tty.check()?;site.before_original_sql(&scope)?;target.require()?;
                if cancelled()||dir_id!=identity((&directory,mount))?||directory.metadata()?.mode()&0o2000!=0||
                    fstatvfs(&directory)?.f_flag.intersects(StatVfsMountFlags::RDONLY|StatVfsMountFlags::NOEXEC){
                    return Err(denied());
                }
                Ok(())
            };
            require()?;absent(&directory)?;
            tty.confirm_image_installation(format!("只写入一个可独立运行的 ZXS 文件：{:?}\nSHA-256 {} / {} 字节；不覆盖、不启动、不设置账户、不创建或迁移任何池。\n",
                path,hex(&digest),asset.len()))?;
            watch_cancellation()?;
            let deadline=Instant::now()+Duration::from_mins(10);
            let alive=||->io::Result<()>{
                if cancelled()||Instant::now()>=deadline{return Err(denied());}
                tty.check()
            };
            alive()?;require()?;absent(&directory)?;
            // No named fallback or cleanup path. This is one new unlinked
            // inode on the admitted parent mount and cannot overwrite anything.
            let file=File::from(openat2(&directory,c".",OFlags::TMPFILE|OFlags::RDWR|OFlags::CLOEXEC,
                Mode::RUSR|Mode::WUSR,WITHIN)?);
            let born=identity((&file,mount))?;
            let shape=|perm:u32,bytes:u64|->io::Result<()>{
                let m=file.metadata()?;
                if (m.uid(),m.gid(),m.mode(),m.nlink(),m.len())!=(0,0,0o100_000|perm,0,bytes)||
                    identity((&file,mount))?!=born{return Err(denied());}
                for key in [c"system.posix_acl_access",c"security.capability"]{no_attribute(&file,key)?;}
                Ok(())
            };
            shape(0o600,0)?;
            let mut start=0u64;
            for block in asset.chunks(512<<10){
                alive()?;
                file.write_all_at(block,start)?;start+=block.len() as u64;
            }
            shape(0o600,asset.len() as u64)?;
            let mut offset=0u64;let mut back=[0;64<<10];
            for wanted in asset.chunks(back.len()){
                alive()?;
                file.read_exact_at(&mut back[..wanted.len()],offset)?;
                if &back[..wanted.len()]!=wanted{return Err(denied());}
                offset+=wanted.len() as u64;
            }
            if file.read_at(&mut [0],offset)?!=0{return Err(denied());}
            fchmod(&file,Mode::from_bits_retain(0o755))?;file.sync_all()?;
            require()?;absent(&directory)?;shape(0o755,asset.len() as u64)?;alive()?;
            // EMPTY_PATH publishes that same exact inode, never identifies a
            // source by its name. EEXIST is a definite non-publication only.
            match linkat(&file,c"",&directory,DEST,AtFlags::EMPTY_PATH){
                Ok(())=>{},
                Err(rustix::io::Errno::EXIST)=>return Err(io::Error::new(io::ErrorKind::AlreadyExists,
                    "another file occupies the chosen name; nothing was overwritten")),
                Err(e)=>return Err(uncertain(e.into())),
            }
            let finish=(||->io::Result<()>{
                let m=file.metadata()?;
                if (m.uid(),m.gid(),m.mode(),m.nlink(),m.len())!=(0,0,0o100_755,1,asset.len() as u64)||
                    identity((&file,mount))?!=born{return Err(denied());}
                file.sync_all()?;
                require()?;
                let installed=Image::select(&path,&digest,product)?;
                if installed.entity()!=born||installed.mount()!=mount||installed.includes(protected){return Err(denied());}
                installed.require_same_published(&file)?;
                directory.sync_all()?;
                require()?;alive()?;
                // Keep the creator's actual post-link file until the final
                // independent stamp and named checked reader both agree.
                installed.require_same_published(&file)?;
                Ok(())
            })();
            finish.map_err(uncertain)?;
            tty.report(&format!("已写入并核对 ZXS 文件 {:?}，SHA-256 {}。没有创建站点、设置账户或启动服务。\n",
                path,hex(&digest))).map_err(uncertain)
        }
    }
    fn uncertain(e:io::Error)->io::Error{
        io::Error::new(e.kind(),format!("ZXS 文件发布结果未确认；保留原目标，不能盲目重试或删除: {e}"))
    }
}

#[cfg(feature="embedded-zxs")]
pub(crate) use enabled::EmbeddedInstall;

#[cfg(test)]
mod tests{
    use super::*;
    use crate::sqlite_install::pair::fresh_issuer::image::tests::small;
    #[test]
    fn embedded_asset_default_has_no_install_command_or_placeholder(){
        if !cfg!(feature="embedded-zxs"){
            let args=["install-zxs-image","--destination-dir","/fixture/opt/zxs",
                "--zxs-sha256","9999999999999999999999999999999999999999999999999999999999999999"];
            assert!(crate::parse(args.map(std::ffi::OsString::from)).is_err());
        }
    }
    #[test]
    fn embedded_asset_parse_cannot_select_a_second_path_or_execution(){
        let args=["--destination-dir","/fixture/selected/bin","--zxs-sha256",
            "9999999999999999999999999999999999999999999999999999999999999999"];
        let parse=|a:&[&str]|super::enabled::EmbeddedInstall::parse(a.iter().map(std::ffi::OsString::from));
        assert_eq!(parse(&args).unwrap().selected_directory(),std::path::Path::new("/fixture/selected/bin"));
        assert_eq!(parse(&args[2..]).unwrap().selected_directory(),std::path::Path::new("/zxs/bin"));
        assert!(parse(&[]).is_err());
        for wrong in ["/","../bin","/zxs","/zxs/state/own","/zxs/log/bin","/zxs/disks/data/F/bin","/fixture/../other","/fixture/bin/"]{
            let mut changed=args;changed[1]=wrong;assert!(parse(&changed).is_err());
        }
        for extra in ["--start","--replace","--credential","--source-path"]{
            let mut changed=args.to_vec();changed.push(extra);assert!(parse(&changed).is_err());
        }
    }
    #[test]
    fn embedded_asset_requires_actual_full_bytes_shape_and_both_expected_digests(){
        let original=small();let expected=Sha256::digest(&original).into();
        assert_eq!(verify(&original,expected,expected).unwrap(),expected);
        assert!(verify(&original,expected,[0;32]).is_err());
        let mut more=original.clone();more.push(0);
        assert!(verify(&more,expected,expected).is_err());
        let mut other=original;other[0]=0;
        let nonelf=Sha256::digest(&other).into();
        assert!(verify(&other,nonelf,nonelf).is_err());
    }
}
