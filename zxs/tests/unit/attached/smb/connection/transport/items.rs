pub(super) trait FixtureIo:Read+Write+Send+Sync {}

impl<T:Read+Write+Send+Sync> FixtureIo for T {}
