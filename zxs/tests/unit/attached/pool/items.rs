pub(crate) fn reject_unconfirmed_for_test(system:&Path,confirm:impl FnOnce(&HeldDir,&fs::OriginalStamp)->io::Result<()>)->io::Result<()>{
    let dir=HeldDir::open(system)?;dir.share()?;
    published_with_sync(&dir,confirm)?.ok_or_else(||invalid("expected original SYSTEM register"))?;
    Ok(())
}
