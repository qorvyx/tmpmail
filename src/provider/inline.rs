//! A message's inline (`cid:`) images under the per-message limits, kept by
//! the part they resolve to. One resolver serves `read --html`, the TUI's
//! `o` and the desktop app, so all three show the same images under the
//! same limits.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::source::{
    MAX_INLINE_CIDS, MAX_INLINE_IMAGE, MAX_INLINE_TOTAL, image_type, inline_images,
};
use super::{Attachment, Message, inline_attachment};

/// The part an inline image comes from.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Part {
    /// A listed attachment, by its index in [`Message::attachments`].
    Listed(usize),
    /// A part of the raw source, by its `Content-ID`.
    Source(String),
}

/// What [`InlineImages::resolve`] needs before it knows a `cid:` name's
/// image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InlineStep {
    /// The name is resolved (to an image or to none): [`InlineImages::serve`]
    /// answers for it now.
    Done,
    /// Download this listed attachment (an index into
    /// [`Message::attachments`]) and hand it to
    /// [`InlineImages::attachment_fetched`], then resolve the name again.
    FetchAttachment(usize),
    /// Read the message's raw source (where the provider has one) and hand
    /// it to [`InlineImages::source_fetched`], then resolve the name again.
    FetchSource,
}

/// What [`InlineImages::serve`] knows of a `cid:` name.
#[derive(Debug, Clone)]
pub enum InlineLookup {
    /// Its type and bytes, counted against the message's limit.
    Image(String, Arc<Vec<u8>>),
    /// None to show: not found, or past the message's limits.
    Missing,
    /// Not resolved yet ([`InlineImages::resolve`]).
    Unknown,
}

/// The inline images of one message, under its limits:
///
/// - images are kept by the part they resolve to, so any number of `cid:`
///   names for one attachment fetch and keep it once;
/// - at most [`MAX_INLINE_CIDS`] distinct names are looked up;
/// - an image over [`MAX_INLINE_IMAGE`] isn't shown, and at most
///   [`MAX_INLINE_TOTAL`] bytes are kept;
/// - at most [`MAX_INLINE_TOTAL`] bytes are shown for each load of the
///   document, counting each time an image is shown
///   ([`InlineImages::serve`], [`InlineImages::new_document`]).
///
/// A name resolves to the attachment the provider lists under it
/// ([`inline_attachment`]), else to the part with that `Content-ID` in the
/// raw source (read once for all names). The resolver fetches nothing
/// itself: [`InlineImages::resolve`] says what to fetch, so a caller can
/// fetch without holding a lock, and [`InlineImages::fetch_all`] does it all
/// in one go.
#[derive(Debug, Default)]
pub struct InlineImages {
    kept: HashMap<Part, (String, Arc<Vec<u8>>)>,
    kept_bytes: usize,
    /// The names resolved so far, and the part each one is, if any.
    cids: HashMap<String, Option<Part>>,
    served: usize,
    /// Listed attachments too big to show (alone or past the limit): not
    /// fetched again while the message is open.
    too_big: HashSet<usize>,
    /// Listed attachments whose download failed for this load of the
    /// document: tried again after [`InlineImages::new_document`].
    unreachable: HashSet<usize>,
    /// The raw source was read (or couldn't be): not fetched again.
    source_read: bool,
    /// The raw source couldn't be read: tried again after
    /// [`InlineImages::new_document`].
    source_failed: bool,
    /// The names served at least once in this load of the document.
    shown: HashSet<String>,
    /// How many names [`InlineImages::fetch_all`] looked up.
    named: usize,
}

/// The inline images of a message's raw source, read without the resolver
/// ([`InlineImages::read_source`]), so a caller can do the decoding without
/// holding a lock, then hand them over ([`InlineImages::add_source`]).
#[derive(Debug, Default)]
pub struct SourceImages {
    /// By name, in the order the HTML shows them; `None` when the source
    /// couldn't be read.
    images: Option<Vec<(String, String, Vec<u8>)>>,
}

impl InlineImages {
    pub fn new() -> Self {
        Self::default()
    }

    fn fits(&self, len: usize) -> bool {
        len <= MAX_INLINE_IMAGE && self.kept_bytes + len <= MAX_INLINE_TOTAL
    }

    /// Resolves `cid` (an id as [`super::cid_of_url`] gives it) for `msg`
    /// as far as what is fetched so far allows.
    pub fn resolve(&mut self, msg: &Message, cid: &str) -> InlineStep {
        if self.cids.contains_key(cid) || self.cids.len() >= MAX_INLINE_CIDS {
            return InlineStep::Done;
        }
        let listed = inline_attachment(msg, cid)
            .and_then(|(att, _)| msg.attachments.iter().position(|a| std::ptr::eq(a, att)));
        if let Some(i) = listed
            && self.kept.contains_key(&Part::Listed(i))
        {
            self.cids.insert(cid.to_string(), Some(Part::Listed(i)));
            return InlineStep::Done;
        }
        // Already read from the source (for another name): not downloaded.
        let part = Part::Source(cid.to_string());
        if self.kept.contains_key(&part) {
            self.cids.insert(cid.to_string(), Some(part));
            return InlineStep::Done;
        }
        if let Some(i) = listed {
            let att = &msg.attachments[i];
            if !self.too_big.contains(&i) && !self.unreachable.contains(&i) {
                // Not fetched when the listing already says it won't fit.
                let listed = att.size.and_then(|s| usize::try_from(s).ok());
                if listed.is_some_and(|len| !self.fits(len)) {
                    self.too_big.insert(i);
                } else {
                    return InlineStep::FetchAttachment(i);
                }
            }
        }
        if !self.source_read {
            return InlineStep::FetchSource;
        }
        self.cids.insert(cid.to_string(), None);
        InlineStep::Done
    }

    /// The download of `msg`'s listed attachment `index` (`None` if it
    /// failed), as [`InlineStep::FetchAttachment`] asked.
    pub fn attachment_fetched(&mut self, msg: &Message, index: usize, bytes: Option<Vec<u8>>) {
        let Some(att) = msg.attachments.get(index) else {
            return;
        };
        let Some(bytes) = bytes else {
            self.unreachable.insert(index);
            return;
        };
        match image_type(&att.content_type, &att.filename) {
            Some(mime) if self.fits(bytes.len()) => {
                self.kept_bytes += bytes.len();
                self.kept
                    .insert(Part::Listed(index), (mime, Arc::new(bytes)));
            }
            _ => {
                self.too_big.insert(index);
            }
        }
    }

    /// `msg`'s raw source (`None` if the provider has none or it couldn't be
    /// read), as [`InlineStep::FetchSource`] asked: [`InlineImages::read_source`]
    /// and [`InlineImages::add_source`] in one.
    pub fn source_fetched(&mut self, msg: &Message, raw: Option<&[u8]>) {
        self.add_source(Self::read_source(msg, raw));
    }

    /// The images of `msg`'s raw source (`None` if the provider has none or
    /// it couldn't be read) for the names its HTML shows, decoded once.
    pub fn read_source(msg: &Message, raw: Option<&[u8]>) -> SourceImages {
        let Some(raw) = raw else {
            return SourceImages { images: None };
        };
        let html = msg.html.as_deref().unwrap_or_default();
        SourceImages {
            images: Some(source_images(&crate::render::cid_images(html), raw)),
        }
    }

    /// Keeps the images [`InlineImages::read_source`] read, in the order
    /// the HTML shows them, while they fit.
    pub fn add_source(&mut self, source: SourceImages) {
        self.source_read = true;
        let Some(images) = source.images else {
            self.source_failed = true;
            return;
        };
        self.source_failed = false;
        for (cid, mime, bytes) in images {
            let part = Part::Source(cid);
            if !self.kept.contains_key(&part) && self.fits(bytes.len()) {
                self.kept_bytes += bytes.len();
                self.kept.insert(part, (mime, Arc::new(bytes)));
            }
        }
    }

    /// Resolves every `cid:` name `msg`'s HTML shows (up to
    /// [`MAX_INLINE_CIDS`]), fetching what is needed with
    /// `fetch_attachment` and `fetch_raw` (each `None` when it fails; the
    /// raw source is asked for at most once).
    pub fn fetch_all(
        &mut self,
        msg: &Message,
        mut fetch_attachment: impl FnMut(&Attachment) -> Option<Vec<u8>>,
        mut fetch_raw: impl FnMut() -> Option<Vec<u8>>,
    ) {
        let Some(html) = msg.html.as_deref() else {
            return;
        };
        let names = crate::render::cid_images(html);
        self.named = names.len().min(MAX_INLINE_CIDS);
        for cid in names.iter().take(MAX_INLINE_CIDS) {
            // Each fetch settles one attachment or the source, so a name
            // takes at most two of them (its listed attachment, then the
            // source) before it is done.
            for _ in 0..3 {
                match self.resolve(msg, cid) {
                    InlineStep::Done => break,
                    InlineStep::FetchAttachment(i) => {
                        let bytes = fetch_attachment(&msg.attachments[i]);
                        self.attachment_fetched(msg, i, bytes);
                    }
                    InlineStep::FetchSource => {
                        let raw = fetch_raw();
                        let images = raw.map(|raw| source_images(&names, &raw));
                        self.add_source(SourceImages { images });
                    }
                }
            }
        }
    }

    /// The image of `cid` for the document, counted against this load's
    /// limit when there is one to show.
    pub fn serve(&mut self, cid: &str) -> InlineLookup {
        match self.cids.get(cid) {
            Some(Some(part)) => match self.kept.get(part) {
                Some((mime, bytes)) if self.served + bytes.len() <= MAX_INLINE_TOTAL => {
                    self.served += bytes.len();
                    if !self.shown.contains(cid) {
                        self.shown.insert(cid.to_string());
                    }
                    InlineLookup::Image(mime.clone(), bytes.clone())
                }
                _ => InlineLookup::Missing,
            },
            Some(None) => InlineLookup::Missing,
            None if self.cids.len() >= MAX_INLINE_CIDS => InlineLookup::Missing,
            None => InlineLookup::Unknown,
        }
    }

    /// Starts the count of a new load of the document (the desktop app's
    /// frame reloads it when the theme or a view option changes): what it
    /// shows counts again, the names that found nothing are resolved again,
    /// and a failed download is tried again; a too big attachment stays
    /// failed.
    pub fn new_document(&mut self) {
        self.served = 0;
        self.shown.clear();
        self.unreachable.clear();
        if self.source_failed {
            self.source_read = false;
        }
        self.cids.retain(|_, part| part.is_some());
    }

    /// How many names resolved to an image.
    pub fn found(&self) -> usize {
        self.cids.values().filter(|p| p.is_some()).count()
    }

    /// How many names had their image shown in this load of the document.
    pub fn shown(&self) -> usize {
        self.shown.len()
    }

    /// How many names [`InlineImages::fetch_all`] looked up (those the HTML
    /// shows, up to [`MAX_INLINE_CIDS`]).
    pub fn named(&self) -> usize {
        self.named
    }

    /// The bytes of the images kept.
    pub fn kept_bytes(&self) -> usize {
        self.kept_bytes
    }

    /// The bytes shown in this load of the document so far.
    pub fn served_bytes(&self) -> usize {
        self.served
    }
}

/// The images of `raw` for `names` (every name the HTML shows: a frame may
/// ask for them in any order), in that order.
fn source_images(names: &[String], raw: &[u8]) -> Vec<(String, String, Vec<u8>)> {
    let mut images = inline_images(raw);
    names
        .iter()
        .filter_map(|cid| {
            let (mime, bytes) = images.remove(cid)?;
            Some((cid.clone(), mime, bytes))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn big_mail(names: usize) -> Message {
        let html: String = (0..names)
            .map(|i| format!("<img src=\"cid:big.png@{i}\">"))
            .collect();
        Message {
            id: "m1".into(),
            html: Some(html),
            attachments: vec![Attachment {
                id: "A1".into(),
                filename: "big.png".into(),
                content_type: "image/png".into(),
                size: None,
                url: None,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn one_attachment_under_many_names_is_fetched_and_kept_once() {
        let msg = big_mail(1000);
        let mut inline = InlineImages::new();
        let mut fetches = 0;
        let mut raws = 0;
        inline.fetch_all(
            &msg,
            |_| {
                fetches += 1;
                Some(vec![0; 9 << 20])
            },
            || {
                raws += 1;
                None
            },
        );
        assert_eq!((fetches, raws), (1, 0));
        assert_eq!(inline.found(), MAX_INLINE_CIDS);
        assert_eq!(inline.named(), MAX_INLINE_CIDS);
        assert_eq!(inline.kept_bytes(), 9 << 20);
        let shown = (0..1000)
            .filter(|i| {
                matches!(
                    inline.serve(&format!("big.png@{i}")),
                    InlineLookup::Image(..)
                )
            })
            .count();
        assert_eq!(shown, MAX_INLINE_TOTAL / (9 << 20));
        assert_eq!(inline.shown(), shown);
        assert!(inline.served_bytes() <= MAX_INLINE_TOTAL);
        inline.new_document();
        assert!(matches!(inline.serve("big.png@0"), InlineLookup::Image(..)));
    }

    #[test]
    fn a_name_already_read_from_the_source_is_not_downloaded() {
        // `a.png` is listed and in the source; `b@x` only in the source,
        // and first in the HTML, so the source is read first.
        let raw = b"Content-Type: multipart/related; boundary=b\r\n\r\n\
--b\r\nContent-Type: text/html\r\n\r\n<img src=cid:b@x><img src=cid:a.png>\r\n\
--b\r\nContent-Type: image/png\r\nContent-ID: <b@x>\r\nContent-Transfer-Encoding: base64\r\n\r\niVBORw0KGgo=\r\n\
--b\r\nContent-Type: image/png\r\nContent-ID: <a.png>\r\nContent-Transfer-Encoding: base64\r\n\r\niVBORw0KGgo=\r\n\
--b--\r\n";
        let msg = Message {
            html: Some("<img src=cid:b@x><img src=cid:a.png>".into()),
            attachments: vec![Attachment {
                id: "a.png".into(),
                filename: "a.png".into(),
                content_type: "image/png".into(),
                size: None,
                url: None,
            }],
            ..Default::default()
        };
        let mut inline = InlineImages::new();
        let (mut downloads, mut raws) = (0, 0);
        inline.fetch_all(
            &msg,
            |_| {
                downloads += 1;
                None
            },
            || {
                raws += 1;
                Some(raw.to_vec())
            },
        );
        assert_eq!((downloads, raws), (0, 1));
        assert_eq!(inline.found(), 2);
    }

    #[test]
    fn too_big_stays_failed_and_unreachable_is_retried() {
        let mut msg = big_mail(1);
        msg.attachments[0].size = Some(11 << 20);
        let mut inline = InlineImages::new();
        // Too big by the listing: not fetched, and the source is tried.
        assert_eq!(inline.resolve(&msg, "big.png@0"), InlineStep::FetchSource);
        inline.source_fetched(&msg, None);
        assert_eq!(inline.resolve(&msg, "big.png@0"), InlineStep::Done);
        // A new load reads the source again (it couldn't be read), but
        // doesn't download the attachment.
        inline.new_document();
        assert_eq!(inline.resolve(&msg, "big.png@0"), InlineStep::FetchSource);
        inline.source_fetched(&msg, None);
        assert_eq!(inline.resolve(&msg, "big.png@0"), InlineStep::Done);
        assert!(matches!(inline.serve("big.png@0"), InlineLookup::Missing));

        let msg = big_mail(1);
        let mut inline = InlineImages::new();
        assert_eq!(
            inline.resolve(&msg, "big.png@0"),
            InlineStep::FetchAttachment(0)
        );
        inline.attachment_fetched(&msg, 0, None);
        assert_eq!(inline.resolve(&msg, "big.png@0"), InlineStep::FetchSource);
        inline.source_fetched(&msg, None);
        assert_eq!(inline.resolve(&msg, "big.png@0"), InlineStep::Done);
        // A new load tries the download again.
        inline.new_document();
        assert_eq!(
            inline.resolve(&msg, "big.png@0"),
            InlineStep::FetchAttachment(0)
        );
    }
}
