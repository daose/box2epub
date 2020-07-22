use box2epub::extractor::BoxnExtractor;
use box2epub::extractor::Extractor;
use futures::future;
use futures::stream::{self, StreamExt};

use epub_builder::EpubBuilder;
use epub_builder::EpubContent;
use epub_builder::ReferenceType;
use epub_builder::ZipLibrary;

// Don't overwhelm the server with too many connections at once
const MAX_PARALLEL: usize = 8;

/// EPUB only accepts xhtml, so this converts html to xhtml (i.e. <br> to <br />)
/// Turns out `prettier` formatting does a pretty good job of this so let's just
/// use this (slow) heavy-handed solution for now.
async fn sanitize_html(html: String) -> String {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;
    use tokio::process::Command;
    let mut prettier_cmd = Command::new("npx")
        .args(vec!["prettier", "--parser", "html"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("Couldn't start npx");
    {
        let stdin = prettier_cmd.stdin.as_mut().unwrap();
        stdin.write_all(html.as_bytes()).await.unwrap();
    }

    String::from_utf8(prettier_cmd.wait_with_output().await.unwrap().stdout)
        .unwrap()
        // TODO: handle html entity conversion properly
        .replace("&nbsp;", "&#160;")
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

    let extractor = BoxnExtractor::new(&site);
    let overview = extractor.extract_overview(&home_html);

    let download_tasks = stream::iter(overview.download_urls.iter().map(|url| {
        let http_client = http_client.clone();
        let url = url.clone();
        let extractor = extractor.clone();
        tokio::spawn(async move {
            println!("Downloading {}", url);
            let chapter_html = http_client
                .get(&url)
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap();
            let mut chapter = extractor.extract_chapter(&chapter_html);
            chapter.content = sanitize_html(chapter.content).await;
            future::ready(chapter).await
        })
    }))
    .buffered(std::cmp::min(MAX_PARALLEL, num_cpus::get()));

    let mut builder = EpubBuilder::new(ZipLibrary::new()?)?;
    builder.metadata("author", overview.author)?;
    builder.metadata("title", overview.title)?;
    if let Some(image_url) = overview.img_url {
        let resp = http_client.get(&image_url).send().await?;
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
            let chapter = task.unwrap();
            let content = {
                if i == 0 {
                    EpubContent::new(&format!("c{}.xhtml", i), chapter.content.as_bytes())
                        .title(chapter.title)
                        // First chapter requires reftype to be set
                        .reftype(ReferenceType::Text)
                } else {
                    EpubContent::new(&format!("c{}.xhtml", i), chapter.content.as_bytes())
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
