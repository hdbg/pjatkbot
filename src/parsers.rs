pub trait IntoLocalized {
    fn localized(&self, locale: &str) -> &str;
}

pub mod pjatk;
