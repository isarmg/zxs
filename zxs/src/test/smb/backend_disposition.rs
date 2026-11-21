//! Table arithmetic only: no forged Core plan, account, or disk mutation.
use super::*;

#[test]
fn only_the_last_real_normal_close_can_move_a_marked_file(){
    use OnClose::*;
    for (live,bound,mark,expected) in [
        (2,1,Unmarked,Departure::Release), // another OPEN merely reserved
        (2,2,Other,Departure::Release),   // the true owner has not closed
        (2,2,Own,Departure::Arm),
        (2,2,Armed,Departure::Release),
        (1,1,Own,Departure::Move),
        (1,1,Armed,Departure::Move),
        (1,1,Unmarked,Departure::Release),
    ]{
        assert_eq!(normal_departure(live,bound,mark).unwrap(),expected);
    }
    for (live,bound,mark) in [(1,0,Own),(2,1,Own),(1,1,Other),(1,1,Retiring)]{
        assert_eq!(normal_departure(live,bound,mark).unwrap_err().kind(),io::ErrorKind::PermissionDenied);
    }
}
