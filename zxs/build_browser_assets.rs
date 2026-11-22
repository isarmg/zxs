//! Verify a reviewed stage as one input; only the fixed response bytes enter the ELF.
use std::{env,fmt::Write as _,fs::{self,File,Metadata},io::{self,Read},
    os::unix::fs::MetadataExt,path::{Path,PathBuf}};

#[path="src/hash.rs"]
mod hash;

const STAGE_ENV:&str="ZXS_BROWSER_ASSETS";
const PIN_ENV:&str="ZXS_BROWSER_ASSET_PIN";
const MAX_BYTES:usize=8*1024*1024;
const DOMAIN:&[u8]=b"ZXS-BROWSER-ASSET-STAGE-1\0";

#[derive(Clone,Copy)]
enum Media{Js,Css,Wasm,Provenance}
impl Media{
    fn content_type(self)->Option<&'static str>{
        match self{
            Self::Js=>Some("text/javascript; charset=utf-8"),
            Self::Css=>Some("text/css; charset=utf-8"),
            Self::Wasm=>Some("application/wasm"),
            Self::Provenance=>None,
        }
    }
    fn require(self,bytes:&[u8])->io::Result<()>{
        match self{
            Self::Wasm if !bytes.starts_with(b"\0asm\x01\0\0\0")=>Err(invalid("browser stage WASM header")),
            Self::Wasm=>Ok(()),
            _=>std::str::from_utf8(bytes).map(|_|()).map_err(|_|invalid("browser stage text is not UTF-8")),
        }
    }
}
const SLOTS:[(&str,Media);5]=[
    ("boot.js",Media::Js),
    ("folder.css",Media::Css),
    ("zxs_browser.js",Media::Js),
    ("zxs_browser_bg.wasm",Media::Wasm),
    ("provenance.json",Media::Provenance),
];

pub(super) fn embed(out:&Path)->io::Result<()>{
    for key in [STAGE_ENV,PIN_ENV]{println!("cargo:rerun-if-env-changed={key}");}
    println!("cargo:rerun-if-changed=build_browser_assets.rs");
    println!("cargo:rerun-if-changed=src/hash.rs");

    let path=env::var(STAGE_ENV).map_err(|_|invalid("ZXS requires a reviewed browser stage (ZXS_BROWSER_ASSETS)"))?;
    if path.chars().any(char::is_control){return Err(invalid("browser stage path contains control characters"));}
    let dir=PathBuf::from(path);
    if !dir.is_absolute()||dir.canonicalize()?.as_os_str()!=dir.as_os_str()||!dir.is_dir(){
        return Err(invalid("browser stage must be one canonical absolute directory"));
    }
    let pin=env::var(PIN_ENV).ok().and_then(|s|pin(&s))
        .ok_or_else(||invalid("ZXS requires one lowercase SHA-256 ZXS_BROWSER_ASSET_PIN"))?;
    println!("cargo:rerun-if-changed={}",dir.display());
    for (name,_) in SLOTS{println!("cargo:rerun-if-changed={}",dir.join(name).display());}
    let bytes=verified_stage(&dir,pin)?;

    let assets=out.join("zxs-browser");
    fs::create_dir_all(&assets)?;
    let mut source=String::from("const ASSETS: &[Asset] = &[\n");
    for ((name,kind),bytes) in SLOTS.into_iter().zip(bytes){
        if let Some(ty)=kind.content_type(){
            fs::write(assets.join(name),bytes)?;
            writeln!(source,"    Asset {{path: \"/ui/v1/{name}\", ty: \"{ty}\", bytes: include_bytes!(\"zxs-browser/{name}\")}},").unwrap();
        }
    }
    source.push_str("];\n");
    fs::write(out.join("browser_assets.rs"),source)
}

fn invalid(message:&'static str)->io::Error{io::Error::new(io::ErrorKind::InvalidData,message)}
fn pin(s:&str)->Option<[u8;32]>{
    let hex=|b:u8|match b{b'0'..=b'9'=>Some(b-b'0'),b'a'..=b'f'=>Some(b-b'a'+10),_=>None};
    if s.len()!=64{return None;}
    let mut out=[0;32];
    for (byte,pair) in out.iter_mut().zip(s.as_bytes().as_chunks::<2>().0){*byte=hex(pair[0])?<<4|hex(pair[1])?;}
    Some(out)
}
fn inventory(dir:&Path)->io::Result<()>{
    let mut found=[false;SLOTS.len()];
    for item in fs::read_dir(dir)?{
        let name=item?.file_name();
        let at=SLOTS.iter().position(|(n,_)|name==*n).ok_or_else(||invalid("unexpected browser stage member"))?;
        if std::mem::replace(&mut found[at],true){return Err(invalid("duplicate browser stage member"));}
    }
    if found.contains(&false){return Err(invalid("incomplete browser stage"));}
    Ok(())
}
fn same(a:&Metadata,b:&Metadata)->bool{
    a.dev()==b.dev()&&a.ino()==b.ino()&&a.mode()==b.mode()&&a.len()==b.len()&&
        (a.mtime(),a.mtime_nsec(),a.ctime(),a.ctime_nsec())==(b.mtime(),b.mtime_nsec(),b.ctime(),b.ctime_nsec())
}
fn read(path:&Path)->io::Result<Vec<u8>>{
    let named=path.symlink_metadata()?;
    if !named.is_file()||!(1..=MAX_BYTES as u64).contains(&named.len()){return Err(invalid("browser stage member type or size"));}
    let mut file=File::open(path)?;
    if !same(&named,&file.metadata()?){return Err(invalid("browser stage member changed before read"));}
    let mut bytes=Vec::new();
    (&mut file).take(MAX_BYTES as u64+1).read_to_end(&mut bytes)?;
    if bytes.len() as u64!=named.len()||!same(&named,&file.metadata()?)||!same(&named,&path.symlink_metadata()?){
        return Err(invalid("browser stage member changed during read"));
    }
    Ok(bytes)
}

fn verified_stage(dir:&Path,expected:[u8;32])->io::Result<[Vec<u8>;SLOTS.len()]>{
    inventory(dir)?;
    let mut files=[const{Vec::new()};SLOTS.len()];
    let mut stage=hash::Sha256::new();
    stage.update(DOMAIN)?;
    for ((name,kind),out) in SLOTS.into_iter().zip(&mut files){
        let bytes=read(&dir.join(name))?;
        kind.require(&bytes)?;
        stage.update(&[name.len() as u8])?;
        stage.update(name.as_bytes())?;
        stage.update(&(bytes.len() as u64).to_be_bytes())?;
        stage.update(&hash::digest(&bytes)?)?;
        *out=bytes;
    }
    inventory(dir)?;
    if stage.finish()!=expected{return Err(invalid("browser stage does not match its independent SHA-256 pin"));}
    Ok(files)
}
