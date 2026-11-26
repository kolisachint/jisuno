//! Line-number scanner for `voice-control-config.json`.
//!
//! `serde_json` discards spans, so [`scan`] walks the raw text with a small
//! JSON-aware scanner (strings are skipped, `\"` escapes honored) and records
//! the 1-based start line of every element of the top-level `commands` array,
//! every element of each command's `steps` array, and every window object in
//! each `workspaces` entry. Non-object elements still record their start line
//! so the vectors stay index-aligned with the parsed `Value`.

use std::collections::BTreeMap;

/// Index-aligned start lines for the arrays validation reports against.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LineMap {
    /// `commands[i]` start line.
    pub commands: Vec<usize>,
    /// `commands[i].steps[j]` start line (`steps[i][j]`).
    pub steps: Vec<Vec<usize>>,
    /// `workspaces[name][k]` start line.
    pub windows: BTreeMap<String, Vec<usize>>,
}

/// Scan `text`, returning element start lines. Never fails: malformed input
/// yields a partial map (check 1 rejects the file before lines are used).
pub fn scan(text: &str) -> LineMap {
    let mut parser = Parser::new(text);
    let mut map = LineMap::default();
    let mut path: Vec<Segment> = Vec::new();
    parser.parse_value(&mut path, &mut map);
    map
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Key(String),
    Index(usize),
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
    line_starts: Vec<usize>,
}

impl<'a> Parser<'a> {
    fn new(text: &'a str) -> Self {
        let bytes = text.as_bytes();
        let mut line_starts = vec![0];
        for (i, &b) in bytes.iter().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        Self {
            bytes,
            pos: 0,
            line_starts,
        }
    }

    fn line_of(&self, pos: usize) -> usize {
        match self.line_starts.binary_search(&pos) {
            Ok(line) => line + 1,
            Err(line) => line,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn parse_value(&mut self, path: &mut Vec<Segment>, map: &mut LineMap) {
        self.skip_ws();
        match self.peek() {
            Some(b'{') => self.parse_object(path, map),
            Some(b'[') => self.parse_array(path, map),
            Some(b'"') => {
                self.parse_string();
            }
            Some(_) => self.parse_literal(),
            None => {}
        }
    }

    fn parse_object(&mut self, path: &mut Vec<Segment>, map: &mut LineMap) {
        if !self.expect(b'{') {
            return;
        }
        // A command object opens a row in `steps` so indices stay aligned
        // even when the command is not an object with a `steps` array.
        if let [Segment::Key(k), Segment::Index(i)] = path.as_slice() {
            if k == "commands" {
                while map.steps.len() <= *i {
                    map.steps.push(Vec::new());
                }
            }
        }
        loop {
            self.skip_ws();
            if self.peek().is_none() {
                return;
            }
            if self.expect(b'}') {
                return;
            }
            let key = match self.peek() {
                Some(b'"') => self.parse_string().unwrap_or_default(),
                _ => return,
            };
            self.skip_ws();
            if !self.expect(b':') {
                return;
            }
            path.push(Segment::Key(key));
            self.parse_value(path, map);
            path.pop();
            self.skip_ws();
            if self.expect(b',') {
                continue;
            }
            if self.expect(b'}') {
                return;
            }
            return;
        }
    }

    fn parse_array(&mut self, path: &mut Vec<Segment>, map: &mut LineMap) {
        if !self.expect(b'[') {
            return;
        }
        // Reset on re-entry so a duplicated key behaves like serde_json
        // (last value wins) instead of appending twice.
        match path.as_slice() {
            [Segment::Key(k)] if k == "commands" => map.commands.clear(),
            [Segment::Key(k), Segment::Index(i), Segment::Key(s)]
                if k == "commands" && s == "steps" =>
            {
                if let Some(steps) = map.steps.get_mut(*i) {
                    steps.clear();
                }
            }
            [Segment::Key(k), Segment::Key(w)] if k == "workspaces" => {
                map.windows.insert(w.clone(), Vec::new());
            }
            _ => {}
        }
        let mut index = 0usize;
        loop {
            self.skip_ws();
            if self.peek().is_none() {
                return;
            }
            if self.expect(b']') {
                return;
            }
            let start = self.line_of(self.pos);
            match path.as_slice() {
                [Segment::Key(k)] if k == "commands" => map.commands.push(start),
                [Segment::Key(k), Segment::Index(i), Segment::Key(s)]
                    if k == "commands" && s == "steps" =>
                {
                    while map.steps.len() <= *i {
                        map.steps.push(Vec::new());
                    }
                    map.steps[*i].push(start);
                }
                [Segment::Key(k), Segment::Key(w)] if k == "workspaces" => {
                    map.windows.entry(w.clone()).or_default().push(start);
                }
                _ => {}
            }
            path.push(Segment::Index(index));
            self.parse_value(path, map);
            path.pop();
            index += 1;
            self.skip_ws();
            if self.expect(b',') {
                continue;
            }
            if self.expect(b']') {
                return;
            }
            return;
        }
    }

    /// Parse a `"..."` string starting at the opening quote. Returns its
    /// contents with escapes resolved minimally (only `\"` and `\\` matter
    /// for scanning; other escapes are kept verbatim).
    fn parse_string(&mut self) -> Option<String> {
        if !self.expect(b'"') {
            return None;
        }
        let mut out = String::new();
        loop {
            let byte = self.peek()?;
            match byte {
                b'"' => {
                    self.pos += 1;
                    return Some(out);
                }
                b'\\' => {
                    self.pos += 1;
                    match self.peek() {
                        Some(next) => {
                            self.pos += 1;
                            match next {
                                b'"' => out.push('"'),
                                b'\\' => out.push('\\'),
                                b'n' => out.push('\n'),
                                b't' => out.push('\t'),
                                b'r' => out.push('\r'),
                                _ => {
                                    out.push('\\');
                                    out.push(next as char);
                                }
                            }
                        }
                        None => return Some(out),
                    }
                }
                _ => {
                    self.pos += 1;
                    out.push(byte as char);
                }
            }
        }
    }

    /// Skip a number, `true`, `false`, or `null`. Anything else (invalid
    /// JSON) is skipped up to the next structural character so the scanner
    /// always makes progress and never panics.
    fn parse_literal(&mut self) {
        let start = self.pos;
        while let Some(byte) = self.peek() {
            if matches!(
                byte,
                b',' | b']' | b'}' | b'"' | b' ' | b'\t' | b'\n' | b'\r'
            ) {
                break;
            }
            self.pos += 1;
        }
        if self.pos == start {
            self.pos += 1;
        }
    }
}
