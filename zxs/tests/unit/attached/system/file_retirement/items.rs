pub(super) fn one_for_test(src:&InitialReader,point:impl Fn(RetireCut)->io::Result<()>)->io::Result<()>{
    let t=next(src,0)?.ok_or_else(||invalid("no pending original retirement"))?;
    run(src,&t,&|_,_,_,_|Ok(()),&||Ok(()),&point)
}
