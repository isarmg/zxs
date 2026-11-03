use service_stack::util::rng::Rng;
use std::{
    ops::Range,
    sync::{Arc, Mutex},
};

#[derive(Debug, PartialEq)]
enum Draw {
    Integer,
    Float,
    Range(Range<u64>),
}

#[derive(Default)]
struct Overrides {
    draws: Arc<Mutex<Vec<Draw>>>,
}

impl Rng for Overrides {
    fn next_u64(&mut self) -> u64 {
        self.draws.lock().unwrap().push(Draw::Integer);
        0
    }

    fn next_f64(&mut self) -> f64 {
        self.draws.lock().unwrap().push(Draw::Float);
        0.75
    }

    fn next_range(&mut self, range: Range<u64>) -> u64 {
        let result = range.end - 1;
        self.draws.lock().unwrap().push(Draw::Range(range));
        result
    }
}

#[test]
fn boxed_concrete_rng_forwards_integer() {
    let mut rng = Box::<Overrides>::default();
    assert_eq!(Rng::next_u64(&mut rng), 0);
    assert_eq!(*rng.draws.lock().unwrap(), [Draw::Integer]);
}

#[test]
fn boxed_concrete_rng_forwards_float_override() {
    let mut rng = Box::<Overrides>::default();
    assert_eq!(Rng::next_f64(&mut rng), 0.75);
    assert_eq!(*rng.draws.lock().unwrap(), [Draw::Float]);
}

#[test]
fn boxed_concrete_rng_forwards_range_override() {
    let mut rng = Box::<Overrides>::default();
    assert_eq!(Rng::next_range(&mut rng, 11..21), 20);
    assert_eq!(*rng.draws.lock().unwrap(), [Draw::Range(11..21)]);
}

fn assert_overrides<R: Rng>(mut rng: R, draws: &Mutex<Vec<Draw>>) {
    assert_eq!(rng.next_u64(), 0);
    assert_eq!(rng.next_f64(), 0.75);
    assert_eq!(rng.next_range(11..21), 20);
    assert_eq!(
        *draws.lock().unwrap(),
        [Draw::Integer, Draw::Float, Draw::Range(11..21)]
    );
}

#[test]
fn boxed_dyn_rng_preserves_overrides() {
    let rng = Overrides::default();
    let draws = Arc::clone(&rng.draws);
    let rng: Box<dyn Rng> = Box::new(rng);
    assert_overrides(rng, &draws);
}

#[test]
fn nested_boxed_rng_preserves_overrides() {
    let rng = Overrides::default();
    let draws = Arc::clone(&rng.draws);
    let rng: Box<dyn Rng + Send + Sync> = Box::new(rng);
    assert_overrides(Box::new(rng), &draws);
}

struct Defaults(u64);

impl Rng for Defaults {
    fn next_u64(&mut self) -> u64 {
        self.0
    }
}

#[test]
fn boxed_rng_preserves_trait_defaults() {
    for value in [0, 1, u64::MAX / 2, u64::MAX] {
        let mut direct = Defaults(value);
        let mut boxed: Box<dyn Rng> = Box::new(Defaults(value));
        assert_eq!(Rng::next_u64(&mut boxed), direct.next_u64());
        assert_eq!(Rng::next_f64(&mut boxed), direct.next_f64());
        for range in [0..1, 11..21, 0..u64::MAX, u64::MAX - 1..u64::MAX] {
            assert_eq!(
                Rng::next_range(&mut boxed, range.clone()),
                direct.next_range(range)
            );
        }
    }
}

#[cfg(feature = "balance")]
#[test]
fn balance_uses_overridden_range_after_boxing() {
    use service_stack::{balance::p2c::Balance, discover::ServiceList, load::Constant};
    use service_stack::{service_fn, Service, ServiceExt};
    use std::{
        convert::Infallible,
        future::{ready, Future},
        task::{Context, Poll, Waker},
    };

    let rng = Overrides::default();
    let draws = Arc::clone(&rng.draws);
    let services = [0_u8, 1, 2]
        .map(|id| Constant::new(service_fn(move |()| ready(Ok::<_, Infallible>(id))), 0));
    let mut balance = Balance::from_rng(ServiceList::new(services), rng);
    let mut context = Context::from_waker(Waker::noop());
    let mut readiness = Box::pin(balance.ready());
    let service = match readiness.as_mut().poll(&mut context) {
        Poll::Ready(Ok(service)) => service,
        other => panic!("expected ready service: {other:?}"),
    };
    assert_eq!(
        *draws.lock().unwrap(),
        [Draw::Range(0..2), Draw::Range(0..3)]
    );
    let mut response = Box::pin(service.call(()));
    assert!(matches!(
        response.as_mut().poll(&mut context),
        Poll::Ready(Ok(1))
    ));
}
