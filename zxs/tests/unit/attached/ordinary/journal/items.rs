impl Journal {
    pub(super) fn intervals(&self,id:FileId)->impl DoubleEndedIterator<Item=&Saved>{
        // Test-only whole-history audit. Production callers must specify a
        // current working cut or the original confirmed submission instead.
        self.spans.get(&id).into_iter().flatten().map(move |&i|match &self.decisions[i]{
            Decision::Span(s)=>s,_=>unreachable!("only admitted working decisions enter this per-FILE index"),
        })
    }
}
