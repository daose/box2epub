mod boxn;
pub use boxn::BoxnExtractor;

pub struct Overview {
    pub title: String,
    pub author: String,
    pub img_url: Option<String>,
    pub download_urls: Vec<String>,
}

pub struct Chapter {
    pub title: String,
    pub content: String,
}

pub trait Extractor {
    fn extract_overview(&self, html: &str) -> Overview;
    fn extract_chapter(&self, html: &str) -> Chapter;
}
