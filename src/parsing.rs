use types::Class;

pub trait IntoLocalized {
    fn localized(&self, locale: &str) -> &str;
}

pub trait ScheduleParser: Send + Sync + 'static {
    const NAME: &'static str;
    fn parse_day(
        &mut self,
        day: chrono::NaiveDate,
    ) -> impl std::future::Future<Output = eyre::Result<Vec<Class>>> + Send;
}

pub mod types;

pub mod manager;

pub mod pjatk;
