use crate::extractor::Extractor;
use crate::extractor::{Chapter, Overview};
use regex::{Regex, RegexBuilder};

use scraper::Selector;

lazy_static! {
    // TODO: regex breaks if more classes are added
    static ref HOME_TITLE_REGEX: Regex =
        RegexBuilder::new(r#"<ol class="breadcrumb">.*<li>.*?<a.*?>(.+?)</a>.*?</li>.*?</ol>"#)
        .dot_matches_new_line(true)
        .build()
        .unwrap();
    static ref HOME_AUTHOR_REGEX: Regex =
        RegexBuilder::new(r#"<div.+?class="author-content".*?>.*?<a.*?>(.+?)</a>"#)
        .dot_matches_new_line(true)
        .build()
        .unwrap();
    static ref HOME_IMAGE_REGEX: Regex =
        RegexBuilder::new(r#"<div.+?class="summary_image">.*?src="(.+?)".*?</div>"#)
        .dot_matches_new_line(true)
        .build()
        .unwrap();

    static ref TITLE_SELECTOR: Selector = Selector::parse("title").unwrap();
    static ref CONTENT_SELECTOR: Selector = Selector::parse("div.text-left").unwrap();
}

#[derive(Clone)]
pub struct BoxnExtractor {
    site: String,
}

impl BoxnExtractor {
    pub fn new(site: &str) -> Self {
        BoxnExtractor {
            site: site.to_string(),
        }
    }
}

impl Extractor for BoxnExtractor {
    fn extract_overview(&self, html: &str) -> Overview {
        let title = HOME_TITLE_REGEX
            .captures(html)
            .map_or("no_title", |capture| capture.get(1).unwrap().as_str())
            .trim()
            .to_string();
        let author = HOME_AUTHOR_REGEX
            .captures(html)
            .map_or("no_author", |capture| capture.get(1).unwrap().as_str())
            .trim()
            .to_string();
        let img_url = HOME_IMAGE_REGEX
            .captures(html)
            .map(|capture| capture.get(1).unwrap().as_str().trim().to_string());

        // TODO: use selectors instead, breaks if novel is also part of popular sidebar
        let chapter_url_regex =
            Regex::new(&format!(r#"<a.+?href="({}.+?)".*?>"#, self.site)).unwrap();
        let mut download_urls: Vec<String> = chapter_url_regex
            .captures_iter(&html)
            .map(|capture| capture.get(1).unwrap().as_str().to_string())
            .collect();
        // reverse because regex collects in newest to oldest but we want oldest to newest
        download_urls.reverse();

        Overview {
            title,
            author,
            img_url,
            download_urls,
        }
    }

    fn extract_chapter(&self, html: &str) -> Chapter {
        let document = scraper::Html::parse_document(html);
        let title_element = document
            .select(&TITLE_SELECTOR)
            .next()
            .expect("No <title> found");
        let title: String = title_element.text().collect();

        let content_element = document
            .select(&CONTENT_SELECTOR)
            .next()
            .expect("No chapter content found");

        let content = format!(
            r#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
    <head>
        <title>{}</title>
    </head>
    <body>
        {}
    </body>
</html>"#,
            title,
            content_element.inner_html()
        );

        Chapter { title, content }
    }
}
