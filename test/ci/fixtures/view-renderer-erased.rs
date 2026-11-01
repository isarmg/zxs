mod erased_safety_regressions {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    struct DropWitness(Arc<AtomicUsize>);

    impl Drop for DropWitness {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    macro_rules! check_erased_container {
        ($module:ident, $container:ty) => {
            mod $module {
                use super::*;

                #[test]
                #[should_panic(expected = "Erased: type mismatch")]
                fn shared_retrieval_rejects_wrong_type() {
                    let value = <$container>::new(42_u64);
                    let _ = value.get_ref::<i64>();
                }

                #[test]
                #[should_panic(expected = "Erased: type mismatch")]
                fn mutable_retrieval_rejects_wrong_type() {
                    let mut value = <$container>::new(42_u64);
                    let _ = value.get_mut::<i64>();
                }

                #[test]
                #[should_panic(expected = "Erased: type mismatch")]
                fn owned_retrieval_rejects_wrong_type() {
                    let value = <$container>::new(42_u64);
                    let _ = value.into_inner::<i64>();
                }

                #[test]
                fn exact_type_supports_read_write_and_move() {
                    let mut value = <$container>::new(String::from("before"));
                    assert_eq!(value.get_ref::<String>(), "before");
                    value.get_mut::<String>().push_str(" after");
                    assert_eq!(value.into_inner::<String>(), "before after");
                }

                #[test]
                fn value_is_destroyed_once_after_drop_or_move() {
                    let drops = Arc::new(AtomicUsize::new(0));
                    drop(<$container>::new(DropWitness(Arc::clone(&drops))));
                    assert_eq!(drops.load(Ordering::Relaxed), 1);

                    let witness = <$container>::new(DropWitness(Arc::clone(&drops)))
                        .into_inner::<DropWitness>();
                    assert_eq!(drops.load(Ordering::Relaxed), 1);
                    drop(witness);
                    assert_eq!(drops.load(Ordering::Relaxed), 2);
                }
            }
        };
    }

    check_erased_container!(send, view_renderer::erased::Erased);
    check_erased_container!(local, view_renderer::erased::ErasedLocal);
}
