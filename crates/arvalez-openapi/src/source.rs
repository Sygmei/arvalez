use std::collections::HashMap;
use std::sync::OnceLock;

use serde_json::Value;

use crate::document::OpenApiDocument;

#[derive(Debug)]
pub(crate) struct LoadedOpenApiDocument {
    pub(crate) document: OpenApiDocument,
    pub(crate) source: OpenApiSource,
}

#[derive(Debug)]
pub(crate) struct OpenApiSource {
    pub(crate) format: SourceFormat,
    pub(crate) raw: String,
    pub(crate) value: OnceLock<Option<Value>>,
    /// Exact pointer → 1-based line map, built lazily from the YAML event stream.
    /// `None` means the crate is JSON (uses heuristic instead) or YAML parsing failed.
    pub(crate) line_map: OnceLock<Option<HashMap<String, usize>>>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum SourceFormat {
    Json,
    Yaml,
}

impl OpenApiSource {
    pub(crate) fn new(format: SourceFormat, raw: String) -> Self {
        Self {
            format,
            raw,
            value: OnceLock::new(),
            line_map: OnceLock::new(),
        }
    }

    fn render_pointer_preview(&self, pointer: &str) -> Option<String> {
        let node = self
            .value
            .get_or_init(|| self.parse_value())
            .as_ref()?
            .pointer(pointer.strip_prefix('#').unwrap_or(pointer))?;
        let rendered = match self.format {
            SourceFormat::Json => serde_json::to_string_pretty(node).ok()?,
            SourceFormat::Yaml => serde_yaml::to_string(node).ok()?,
        };
        Some(truncate_preview(&rendered, 10))
    }

    /// Return `(preview_string, 1_based_line)` for the node at `pointer`.
    ///
    /// For YAML sources both values are derived together: we look up the exact
    /// key line from the event-stream map and then slice the raw text from that
    /// line, so the stored line and the start of the preview are always the
    /// same point in the file.  For JSON sources we fall back to the
    /// serde-rendered preview and a text-search heuristic line number.
    pub(crate) fn pointer_info(&self, pointer: &str) -> (Option<String>, Option<usize>) {
        match self.format {
            SourceFormat::Yaml => {
                let key = pointer.strip_prefix('#').unwrap_or(pointer);
                let line = self
                    .line_map
                    .get_or_init(|| Some(build_yaml_line_map(&self.raw)))
                    .as_ref()
                    .and_then(|m| m.get(key).copied());
                let preview = line.map(|l| self.raw_preview_from_line(l));
                (preview, line)
            }
            SourceFormat::Json => {
                let preview = self.render_pointer_preview(pointer);
                let line = self.resolve_pointer_line_heuristic(pointer);
                (preview, line)
            }
        }
    }

    /// Slice `max_lines` raw source lines starting at 1-based `start_line`,
    /// dedented to remove the leading whitespace shared by all lines.
    fn raw_preview_from_line(&self, start_line: usize) -> String {
        const MAX_LINES: usize = 10;
        let lines: Vec<&str> = self
            .raw
            .lines()
            .skip(start_line.saturating_sub(1))
            .take(MAX_LINES)
            .collect();
        // Compute common leading-whitespace indent so the preview isn't
        // rendered with the full nesting depth.
        let indent = lines
            .iter()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.len() - l.trim_start().len())
            .min()
            .unwrap_or(0);
        let dedented: Vec<&str> = lines.iter().map(|l| &l[indent.min(l.len())..]).collect();
        dedented.join("\n")
    }
    /// Text-search heuristic used for JSON sources (or as a last-resort fallback).
    fn resolve_pointer_line_heuristic(&self, pointer: &str) -> Option<usize> {
        let inner = pointer.strip_prefix('#').unwrap_or(pointer);
        let segments: Vec<String> = inner
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| s.replace("~1", "/").replace("~0", "~"))
            .collect();

        let lines: Vec<&str> = self.raw.lines().collect();
        let mut search_from = 0usize;
        let mut last_found: Option<usize> = None;

        for segment in &segments {
            let yaml_pat = format!("{}:", segment);
            let json_pat = format!("\"{}\":", segment);
            for (idx, line) in lines.iter().enumerate().skip(search_from) {
                let trimmed = line.trim();
                if trimmed.starts_with(&yaml_pat) || trimmed.starts_with(&json_pat) {
                    last_found = Some(idx + 1);
                    search_from = idx + 1;
                    break;
                }
            }
        }
        last_found
    }

    fn parse_value(&self) -> Option<Value> {
        match self.format {
            SourceFormat::Json => serde_json::from_str(&self.raw).ok(),
            SourceFormat::Yaml => {
                let yaml_value: serde_yaml::Value = serde_yaml::from_str(&self.raw).ok()?;
                serde_json::to_value(yaml_value).ok()
            }
        }
    }
}

pub(crate) fn truncate_preview(rendered: &str, max_lines: usize) -> String {
    let lines = rendered.lines().collect::<Vec<_>>();
    if lines.len() <= max_lines {
        return rendered.to_owned();
    }

    let mut output = lines
        .into_iter()
        .take(max_lines)
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    output.push("...".into());
    output.join("\n")
}

/// Build an exact JSON-pointer → 1-based-line map by walking the YAML event
/// stream.  Every key scalar's line is recorded under the pointer formed by
/// appending that key (RFC 6901-encoded) to the parent pointer.  This gives
/// precise, heuristic-free location data for any depth, including empty-string
/// keys and numeric array indices.
pub(crate) fn build_yaml_line_map(raw: &str) -> HashMap<String, usize> {
    use yaml_rust2::parser::{Event, MarkedEventReceiver, Parser};
    use yaml_rust2::scanner::Marker;

    enum Frame {
        Mapping {
            ptr: String,
            /// `true`  = next Scalar event is a key
            /// `false` = next event is the value for `pending_key`
            expecting_key: bool,
            pending_key: String,
            /// 1-based line of `pending_key`; used as the stored line for
            /// scalar values (key and value are on the same line).  Complex
            /// values (MappingStart/SequenceStart) use the event's own mark
            /// instead, which points at the first line of rendered content.
            pending_line: usize,
        },
        Sequence {
            ptr: String,
            index: usize,
        },
    }

    struct Collector {
        stack: Vec<Frame>,
        map: HashMap<String, usize>,
    }

    /// RFC 6901 segment encoding (~ → ~0, / → ~1).
    fn enc(key: &str) -> String {
        key.replace('~', "~0").replace('/', "~1")
    }

    impl MarkedEventReceiver for Collector {
        fn on_event(&mut self, ev: Event, mark: Marker) {
            // yaml-rust2 Marker::line() is already 1-based.
            let line = mark.line();

            match ev {
                // ── Mapping / Sequence start ───────────────────────────────
                Event::MappingStart(..) | Event::SequenceStart(..) => {
                    let is_mapping = matches!(ev, Event::MappingStart(..));

                    // Phase 1: derive the child pointer from the parent frame.
                    // We compute owned Strings so the borrow on self.stack ends
                    // before we mutate self.map / self.stack below.
                    let (child_ptr, record_line) = match self.stack.last() {
                        None => (String::new(), None), // root node
                        Some(Frame::Mapping {
                            ptr,
                            expecting_key: false,
                            pending_key,
                            pending_line,
                        }) => {
                            // Record the KEY's line (`pending_line`) so that
                            // `raw_preview_from_line` starts exactly at the key
                            // (e.g. `"":`), which is what users and the frontend
                            // expect to see highlighted.
                            (format!("{}/{}", ptr, enc(pending_key)), Some(*pending_line))
                        }
                        Some(Frame::Sequence { ptr, index }) => {
                            (format!("{}/{}", ptr, index), Some(line))
                        }
                        // expecting_key == true here would mean a nested
                        // structure used as a mapping key — invalid YAML.
                        _ => return,
                    };

                    // Phase 2: record, update parent, push child (no active
                    // borrow on self.stack from this point).
                    if let Some(l) = record_line {
                        self.map.insert(child_ptr.clone(), l);
                    }
                    match self.stack.last_mut() {
                        Some(Frame::Mapping { expecting_key, .. }) => *expecting_key = true,
                        Some(Frame::Sequence { index, .. }) => *index += 1,
                        None => {}
                    }
                    if is_mapping {
                        self.stack.push(Frame::Mapping {
                            ptr: child_ptr,
                            expecting_key: true,
                            pending_key: String::new(),
                            pending_line: 0,
                        });
                    } else {
                        self.stack.push(Frame::Sequence {
                            ptr: child_ptr,
                            index: 0,
                        });
                    }
                }

                // ── Mapping / Sequence end ────────────────────────────────
                Event::MappingEnd | Event::SequenceEnd => {
                    self.stack.pop();
                }

                // ── Scalar ────────────────────────────────────────────────
                Event::Scalar(value, ..) => {
                    // Phase 1: figure out what the scalar represents.
                    let is_key = matches!(
                        self.stack.last(),
                        Some(Frame::Mapping { expecting_key: true, .. })
                    );
                    let value_info: Option<(String, usize)> = if !is_key {
                        match self.stack.last() {
                            Some(Frame::Mapping {
                                ptr,
                                expecting_key: false,
                                pending_key,
                                pending_line,
                            }) => Some((format!("{}/{}", ptr, enc(pending_key)), *pending_line)),
                            Some(Frame::Sequence { ptr, index }) => {
                                Some((format!("{}/{}", ptr, index), line))
                            }
                            _ => None,
                        }
                    } else {
                        None
                    };

                    // Phase 2: apply (borrows above have been released).
                    if is_key {
                        if let Some(Frame::Mapping {
                            expecting_key,
                            pending_key,
                            pending_line,
                            ..
                        }) = self.stack.last_mut()
                        {
                            *pending_key = value;
                            *pending_line = line;
                            *expecting_key = false;
                        }
                    } else if let Some((child_ptr, record_line)) = value_info {
                        self.map.insert(child_ptr, record_line);
                        match self.stack.last_mut() {
                            Some(Frame::Mapping { expecting_key, .. }) => *expecting_key = true,
                            Some(Frame::Sequence { index, .. }) => *index += 1,
                            _ => {}
                        }
                    }
                }

                _ => {}
            }
        }
    }

    let mut collector = Collector {
        stack: Vec::new(),
        map: HashMap::new(),
    };
    let mut parser = Parser::new(raw.chars());
    if parser.load(&mut collector, false).is_err() {
        return HashMap::new();
    }
    collector.map
}
