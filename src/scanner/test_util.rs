//! Scanner test fixtures.

use std::io::Write;
use std::path::Path;

use sqlx::SqlitePool;

use crate::media::identity::{self, Identity};

pub(crate) fn write_cbz(path: &Path, content: &str) {
    let f = std::fs::File::create(path).unwrap();
    let mut z = zip::ZipWriter::new(f);
    let opts = zip::write::SimpleFileOptions::default();
    z.start_file("001.jpg", opts).unwrap();
    z.write_all(format!("dummy-jpeg-{content}").as_bytes())
        .unwrap();
    z.finish().unwrap();
}

pub(crate) fn structural_of(path: &Path) -> String {
    match identity::identify(path).unwrap() {
        Identity::Ready {
            structural_hash, ..
        } => structural_hash,
        Identity::NotReady => panic!("a complete cbz must be Ready"),
    }
}

pub(crate) fn write_epub(path: &Path, title: &str, body: &str) {
    let f = std::fs::File::create(path).unwrap();
    let mut z = zip::ZipWriter::new(f);
    let opts = zip::write::SimpleFileOptions::default();
    let mut put = |name: &str, data: &[u8]| {
        z.start_file(name, opts).unwrap();
        z.write_all(data).unwrap();
    };
    put(
        "META-INF/container.xml",
        br#"<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
    );
    put(
        "content.opf",
        format!(
            r#"<package xmlns="http://www.idpf.org/2007/opf"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>{title}</dc:title></metadata><manifest><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/></spine></package>"#
        )
        .as_bytes(),
    );
    put(
        "c1.xhtml",
        format!("<html><body>{body}</body></html>").as_bytes(),
    );
    z.finish().unwrap();
}

pub(crate) async fn items(pool: &SqlitePool) -> Vec<(i64, String, String, String)> {
    sqlx::query_as("SELECT id, structural_hash, path, kind FROM items ORDER BY path")
        .fetch_all(pool)
        .await
        .unwrap()
}
