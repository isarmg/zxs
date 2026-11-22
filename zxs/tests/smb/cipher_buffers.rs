use super::{InOut, InOutBuf};
use hybrid_array::typenum::U4;

#[test]
fn borrowed_groups_and_tail_cover_exactly_the_selected_prefix() {
    let input = [2_u8, 3, 5, 7, 11, 13, 17, 19, 23, 29];
    for len in [0, 1, 3, 4, 5, 7, 8, 9] {
        let mut data = input;
        let (groups, tail) = InOutBuf::from(&mut data[..len]).into_chunks::<U4>();
        assert_eq!(groups.len(), len / 4);
        assert_eq!(tail.len(), len % 4);

        let mut expected = input.into_iter();
        for mut group in groups {
            for slot in 0..4 {
                let mut value = group.get(slot);
                assert_eq!(value.clone_in(), expected.next().unwrap());
                *value.get_out() = 0;
            }
        }

        let mut rest = tail.into_iter();
        for mut value in rest.by_ref() {
            assert_eq!(*value.get_in(), expected.next().unwrap());
            *value.get_out() = 0;
        }
        assert!(rest.next().is_none());
        assert!(data[..len].iter().all(|&value| value == 0));
        assert_eq!(data[len..], input[len..]);
    }
}

#[test]
fn consuming_the_iterator_keeps_earlier_borrows_on_separate_slots() {
    let mut data = [3_u8, 5, 8];
    let mut iter = InOutBuf::from(data.as_mut_slice()).into_iter();
    let mut first = iter.next().unwrap();
    let mut second = iter.next().unwrap();
    assert_eq!(*first.get_in(), 3);
    *first.get_out() = 13;
    assert_eq!(*second.get_in(), 5);
    *second.get_out() = 21;
    assert_eq!(*first.get_in(), 13);
    assert_eq!(*iter.next().unwrap().get_in(), 8);
    assert!(iter.next().is_none());

    let mut value = InOut::from(&mut data[2]);
    assert_eq!(*value.get_in(), 8);
    *value.get_out() = 34;
    assert_eq!(data, [13, 21, 34]);
}
