use futures::future;
use futures::stream::{self, StreamExt};
use regex::Regex;
use regex::RegexBuilder;
use std::io::Write;

use epub_builder::EpubBuilder;
use epub_builder::EpubContent;
use epub_builder::ReferenceType;
use epub_builder::ZipLibrary;

// Don't overwhelm the server with too many connections at once
const MAX_PARALLEL: usize = 8;

#[macro_use]
extern crate lazy_static;

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
}

/// EPUB only accepts xhtml, so this converts html to xhtml (i.e. <br> to <br />)
/// Turns out `prettier` formatting does a pretty good job of this so let's just
/// use this (slow) heavy-handed solution for now.
///
/// TODO: change from std process to tokio async process
fn sanitize_html(html: String) -> String {
    use std::process::{Command, Stdio};

    let mut prettier_cmd = Command::new("npx")
        .args(vec!["prettier", "--parser", "html"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let stdin = prettier_cmd.stdin.as_mut().unwrap();
        stdin.write_all(html.as_bytes()).unwrap();
    }

    String::from_utf8(prettier_cmd.wait_with_output().unwrap().stdout)
        .unwrap()
        // TODO: handle html entity conversion properly
        .replace("&nbsp;", "&#160;")
}

use scraper::Selector;
lazy_static! {
    static ref TITLE_SELECTOR: Selector = Selector::parse("title").unwrap();
    static ref CONTENT_SELECTOR: Selector = Selector::parse("div.text-left").unwrap();
}

struct Chapter {
    title: String,
    content: String,
}

fn extract_chapter(html: &str) -> Chapter {
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + 'static>> {
    // Normalize the site to have slash at the end
    let site = {
        let raw_site = std::env::args()
            .nth(1)
            .expect("One argument to be provided");
        let last_char = raw_site
            .chars()
            .last()
            .expect("Argument should at least have one character");
        if last_char == '/' {
            raw_site
        } else {
            raw_site + "/"
        }
    };
    let http_client = reqwest::Client::new();
    let home_html = http_client.get(&site).send().await?.text().await?;

    let title = HOME_TITLE_REGEX
        .captures(&home_html)
        .map_or("box2epub", |capture| capture.get(1).unwrap().as_str())
        .trim();
    let author = HOME_AUTHOR_REGEX
        .captures(&home_html)
        .map_or("box2epub", |capture| capture.get(1).unwrap().as_str())
        .trim();
    let image_url_opt = HOME_IMAGE_REGEX
        .captures(&home_html)
        .map(|capture| capture.get(1).unwrap().as_str().trim());

    let chapter_url_regex = Regex::new(&format!(r#"<a.+?href="({}.+?)".*?>"#, site)).unwrap();
    let chapter_urls: Vec<String> = chapter_url_regex
        .captures_iter(&home_html)
        .map(|capture| capture.get(1).unwrap().as_str().to_string())
        .collect();

    // Use rev to reverse since chapter_urls are captured in latest->oldest order
    let download_tasks = stream::iter(chapter_urls.iter().rev().map(|url| {
        let http_client = http_client.clone();
        let url = url.clone();
        tokio::spawn(async move {
            println!("Downloading {}", url);
            http_client.get(&url).send().await?.text().await
        })
    }))
    .buffered(std::cmp::min(MAX_PARALLEL, num_cpus::get()));

    let mut builder = EpubBuilder::new(ZipLibrary::new()?)?;
    builder.metadata("author", author)?;
    builder.metadata("title", title)?;
    if let Some(image_url) = image_url_opt {
        let resp = http_client.get(image_url).send().await?;
        let mimetype_opt = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .map(|v| v.to_str().unwrap().to_owned());
        if let Some(mimetype) = mimetype_opt {
            let image_bytes = resp.bytes().await?;
            if mimetype == "image/png" {
                builder.add_cover_image("cover.png", image_bytes.as_ref(), mimetype)?;
            } else if mimetype == "image/jpeg" {
                builder.add_cover_image("cover.jpg", image_bytes.as_ref(), mimetype)?;
            } else {
                println!("Cover photo mimetype not supported: {}", mimetype);
            }
        }
    }

    builder.inline_toc();

    download_tasks
        .enumerate()
        .for_each(|(i, task)| {
            let chapter_html = task.unwrap().unwrap();
            let chapter = extract_chapter(&chapter_html);
            let chapter_body = sanitize_html(chapter.content);
            let content = {
                if i == 0 {
                    EpubContent::new(&format!("c{}.xhtml", i), chapter_body.as_bytes())
                        .title(chapter.title)
                        // First chapter requires reftype to be set
                        .reftype(ReferenceType::Text)
                } else {
                    EpubContent::new(&format!("c{}.xhtml", i), chapter_body.as_bytes())
                        .title(chapter.title)
                }
            };

            builder.add_content(content).unwrap();

            future::ready(())
        })
        .await;

    let epub_file = std::fs::File::create("output.epub")?;
    builder.generate(epub_file)?;

    Ok(())
}

/*
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test() {
        let chapter_html = std::fs::read_to_string("./assets/chapter.html").unwrap();
        println!("{}", sanitize_html(chapter_html));
        assert!(false);
    }
}
*/
