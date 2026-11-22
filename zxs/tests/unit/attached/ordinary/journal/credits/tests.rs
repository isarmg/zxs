use super::*;
fn deletion(file:u8,op:u8,hash:u8)->Deletion{Deletion{file:FileId([file;16]),operation:OperationId([op;16]),decision:[hash;32]}}
fn retain(k:Deletion)->Effect{Effect::Retain{file:k.file,operation:k.operation}}
#[test]
fn successful_delete_needs_room_for_itself_and_one_worst_size_return_in_both_dimensions(){
    let c=Credits::default();let k=deletion(1,2,3);
    for(n,used)in [(MAX_DECISIONS-1,0),(32,MAX_JOURNAL-2*MAX_FRAME+1)]{
        assert!(c.room(n,used,MAX_FRAME,Effect::Ordinary).is_ok());
        assert!(c.room(n,used,MAX_FRAME,retain(k)).is_err());
    }
    assert!(c.room(MAX_DECISIONS-2,MAX_JOURNAL-2*MAX_FRAME,MAX_FRAME,retain(k)).is_ok());
    assert!(c.room(usize::MAX,0,1,Effect::Ordinary).is_err());
    assert!(c.room(0,usize::MAX,1,Effect::Ordinary).is_err());
    for wrong in [0,MAX_FRAME+1]{assert!(c.room(0,0,wrong,Effect::Ordinary).is_err());}
}
#[test]
fn two_distinct_originals_keep_both_returns_at_each_limit_and_cannot_be_spent_by_an_unrelated_writer(){
    let(a,b)=(deletion(1,10,41),deletion(2,20,42));
    for(n,used)in [(MAX_DECISIONS-2,4096),(100,MAX_JOURNAL-2*MAX_FRAME)]{
        let mut c=Credits::default();c.apply(retain(a),a.decision).unwrap();c.apply(retain(b),b.decision).unwrap();
        assert!(c.room(n,used,MAX_FRAME,Effect::Ordinary).is_err());
        assert!(c.room(n,used,MAX_FRAME,retain(deletion(3,30,43))).is_err());
        for stale in [deletion(1,11,41),deletion(1,10,40),deletion(2,10,41)]{
            assert!(c.room(n,used,MAX_FRAME,Effect::Restore(stale)).is_err());
        }
        c.room(n,used,MAX_FRAME,Effect::Restore(a)).unwrap();c.apply(Effect::Restore(a),[51;32]).unwrap();
        assert!(c.room(n+1,used+MAX_FRAME,MAX_FRAME,Effect::Ordinary).is_err());
        assert!(c.room(n+1,used+MAX_FRAME,1,Effect::Restore(a)).is_err());
        c.room(n+1,used+MAX_FRAME,MAX_FRAME,Effect::Restore(b)).unwrap();
    }
}
#[test]
fn a_complete_but_unconfirmed_last_frame_never_releases_its_credit(){
    let(a,later)=(deletion(1,2,3),deletion(1,4,5));let mut c=Credits::default();
    c.apply(retain(a),a.decision).unwrap();c.before_unconfirmed(retain(a),a.decision).unwrap();
    assert!(c.resulting(Effect::Restore(a)).is_err());
    c.apply(retain(a),a.decision).unwrap();c.apply(Effect::Restore(a),[81;32]).unwrap();
    c.before_unconfirmed(Effect::Restore(a),[81;32]).unwrap();
    assert!(c.resulting(retain(later)).is_err());assert!(c.resulting(Effect::Restore(later)).is_err());
    assert!(c.room(MAX_DECISIONS-1,10,16,Effect::Ordinary).is_err());
    c.room(MAX_DECISIONS-1,10,MAX_FRAME,Effect::Restore(a)).unwrap();
    c.apply(Effect::Restore(a),[81;32]).unwrap();c.apply(retain(later),later.decision).unwrap();
    assert!(c.resulting(Effect::Restore(a)).is_err());
}
