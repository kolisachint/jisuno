//! Voice-command phrase matcher.
//!
//! Matches a transcribed utterance against command phrases that may contain
//! `{slot}` templates, per `docs/design/voice-control.md` §5 ("Slots" and
//! "Matching").
//!
//! Algorithm:
//!
//! * `when` filtering happens first: candidates whose `when_apps` does not
//!   list the frontmost app are removed before matching.
//! * Both sides are normalized (lowercase, punctuation stripped, split into
//!   words; `{slot}` tokens are preserved in phrases).
//! * Each phrase is scored by word-level edit distance. Substituting word `a`
//!   for word `b` costs the character-level normalized edit distance between
//!   them; inserting or deleting a word costs 1. The total is divided by the
//!   rendered phrase length in words.
//! * Fixed-list slots are fuzzy matched like literal words: every list value
//!   is tried (values may be multi-word, e.g. `right-half` matches spoken
//!   "right half") and the best is kept. The filled slot reports the canonical
//!   list value.
//! * A free slot is allowed once per phrase, as the last token only. It
//!   captures the remaining utterance words verbatim (original casing,
//!   trimmed) at zero cost and must capture at least one word.
//! * The best candidate runs when its distance is at most 0.20 and no other
//!   candidate (different command or phrase) is within 0.05 of it; otherwise
//!   the best phrase is returned as a `DidYouMean` suggestion (rendered with
//!   slots filled). `NoMatch` is returned only when there are no eligible
//!   candidates or the utterance is empty.
//!
//! Performance: character-level costs between the utterance words and every
//! distinct phrase/value word are computed once per call into a lookup table,
//! and the dynamic programs reuse scratch buffers, so the inner loop performs
//! no allocations.

use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

/// Best distance that still runs: 20% of the phrase length.
const ACCEPT_DISTANCE: f32 = 0.20;
/// Distance band that turns an accepted match into a near-tie suggestion.
const TIE_DISTANCE: f32 = 0.05;

/// A matchable phrase. `phrase` may contain `{slot}` templates.
#[derive(Debug, Clone, Copy)]
pub struct Candidate<'a> {
    /// Command this phrase belongs to.
    pub command_id: &'a str,
    /// Phrase template, e.g. `"move {window} to {place}"`.
    pub phrase: &'a str,
    /// Bundle ids this command is limited to, or `None` for always eligible.
    pub when_apps: Option<&'a [String]>,
}

/// Possible values for a `{slot}` in a phrase.
#[derive(Debug, Clone, Copy)]
pub enum SlotValues<'a> {
    /// Fixed list; the canonical list entries (e.g. `right-half`).
    List(&'a [String]),
    /// Free text; allowed once per phrase, as the last token only.
    Free,
}

/// Provides slot value lists. Implemented by the config layer; the matcher
/// never reads config files itself.
pub trait SlotSource {
    /// Values for `slot` in `command_id`, or `None` when unknown.
    fn values(&self, command_id: &str, slot: &str) -> Option<SlotValues<'_>>;
}

/// An accepted match.
#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    /// Id of the matched command.
    pub command_id: String,
    /// Phrase rendered with slots filled, e.g. `"move terminal to right half"`.
    pub phrase: String,
    /// Filled slots: canonical list values, or verbatim text for free slots.
    pub slots: BTreeMap<String, String>,
    /// Normalized edit distance of the winning alignment.
    pub distance: f32,
}

/// Result of matching one utterance.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Run the command with `Match::slots`.
    Run(Match),
    /// Nothing runs; suggest this phrase instead.
    DidYouMean {
        /// Best phrase rendered with slots filled.
        phrase: String,
        /// Command the best phrase belongs to.
        command_id: String,
    },
    /// No eligible candidates, or the utterance is empty.
    NoMatch,
}

/// Lowercase, strip punctuation, and split into words.
///
/// `{slot}` tokens are preserved as single tokens so phrase templates survive
/// normalization.
pub fn normalize(text: &str) -> Vec<String> {
    tokenize(text).into_iter().map(|tok| tok.word).collect()
}

/// Match `utterance` against `candidates`; see the module docs for semantics.
pub fn match_utterance(
    utterance: &str,
    frontmost_app: Option<&str>,
    candidates: &[Candidate<'_>],
    slots: &dyn SlotSource,
) -> Outcome {
    let utt = tokenize(utterance);
    if utt.is_empty() {
        return Outcome::NoMatch;
    }
    let eligible: Vec<&Candidate> = candidates
        .iter()
        .filter(|c| match c.when_apps {
            None => true,
            Some(apps) => frontmost_app.is_some_and(|app| apps.iter().any(|a| a.as_str() == app)),
        })
        .collect();
    if eligible.is_empty() {
        return Outcome::NoMatch;
    }

    // Intern the utterance words first so they own ids 0..ucount; everything
    // else follows. Character-level costs are then precomputed once per call.
    let mut vocab = Vocab::default();
    let uids: Vec<u32> = utt.iter().map(|tok| vocab.intern(&tok.word)).collect();
    let ucount = vocab.words.len();

    // Normalize each distinct slot value once (lists repeat across phrases),
    // and share the built value list across phrases reusing one slice.
    let mut value_cache: HashMap<String, (Rc<[u32]>, usize)> = HashMap::new();
    let mut uniq_values: Vec<Rc<[u32]>> = Vec::new();
    let mut list_cache: HashMap<(*const String, usize), Rc<Vec<FixedValue>>> = HashMap::new();
    let mut value_occurrences = 0usize;
    let mut prepared: Vec<Prepared> = Vec::with_capacity(eligible.len());
    for c in &eligible {
        let mut tmpl = Vec::new();
        for tok in &tokenize(c.phrase) {
            if is_slot_token(&tok.word) {
                let name = tok.word[1..tok.word.len() - 1].to_string();
                match slots.values(c.command_id, &name) {
                    Some(SlotValues::List(list)) if !list.is_empty() => {
                        value_occurrences += list.len();
                        let key = (list.as_ptr(), list.len());
                        let values = match list_cache.get(&key) {
                            Some(shared) => Rc::clone(shared),
                            None => {
                                let built: Vec<FixedValue> = list
                                    .iter()
                                    .map(|raw| {
                                        let (ids, uidx) = value_word_idx(
                                            raw,
                                            &mut vocab,
                                            &mut value_cache,
                                            &mut uniq_values,
                                        );
                                        FixedValue {
                                            display: raw.replace('-', " "),
                                            canonical: raw.clone(),
                                            ids,
                                            uidx,
                                        }
                                    })
                                    .collect();
                                let shared = Rc::new(built);
                                list_cache.insert(key, Rc::clone(&shared));
                                shared
                            }
                        };
                        tmpl.push(Pattern::Fixed { name, values });
                    }
                    Some(SlotValues::Free) => tmpl.push(Pattern::Free { name }),
                    _ => tmpl.push(Pattern::Unknown { name }),
                }
            } else {
                tmpl.push(Pattern::Lit(vocab.intern(&tok.word)));
            }
        }
        prepared.push(Prepared {
            command_id: c.command_id,
            template: c.phrase,
            tmpl,
        });
    }
    vocab.build_costs(ucount);

    // Precompute span costs when values repeat across candidates: every
    // utterance span against every distinct fixed-list value, once per call.
    let mut ctx = Ctx::default();
    let spans = SpanTable::build(
        &uids,
        &uniq_values,
        &vocab,
        &mut ctx.word,
        value_occurrences,
    );

    // Phase 1: score every candidate. Alignment fills are compact
    // (indices only) and reused across candidates, so this performs no
    // per-candidate allocations after warmup.
    let mut scored: Vec<Scored> = Vec::with_capacity(prepared.len());
    for (idx, p) in prepared.iter().enumerate() {
        let cost = align(&utt, &uids, &p.tmpl, &vocab, spans.as_ref(), &mut ctx);
        let words = count_words(&utt, utterance, &p.tmpl, &ctx.fills);
        scored.push(Scored {
            distance: cost / words.max(1) as f32,
            idx,
        });
    }

    let mut best = 0;
    for (i, s) in scored.iter().enumerate().skip(1) {
        if s.distance < scored[best].distance {
            best = i;
        }
    }
    let mut second: Option<usize> = None;
    for (i, s) in scored.iter().enumerate() {
        if i == best {
            continue;
        }
        // An identical (command, phrase) entry is not a competing candidate.
        if prepared[s.idx].command_id == prepared[scored[best].idx].command_id
            && prepared[s.idx].template == prepared[scored[best].idx].template
        {
            continue;
        }
        if second.is_none_or(|k| s.distance < scored[k].distance) {
            second = Some(i);
        }
    }
    let tied = second.is_some_and(|k| scored[k].distance - scored[best].distance <= TIE_DISTANCE);
    // Phase 2: realign the winner and build its strings and slot map.
    let winner = &prepared[scored[best].idx];
    let _ = align(&utt, &uids, &winner.tmpl, &vocab, spans.as_ref(), &mut ctx);
    let (slots, rendered) = render(&utt, utterance, &winner.tmpl, &ctx.fills, &vocab);
    let distance = scored[best].distance;
    if !tied && distance <= ACCEPT_DISTANCE {
        Outcome::Run(Match {
            command_id: winner.command_id.to_string(),
            phrase: rendered,
            slots,
            distance,
        })
    } else {
        Outcome::DidYouMean {
            phrase: rendered,
            command_id: winner.command_id.to_string(),
        }
    }
}

/// One normalized word plus its byte span in the source text.
struct Tok {
    word: String,
    start: usize,
    end: usize,
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '{' || c == '}' || c == '_'
}

fn is_slot_token(token: &str) -> bool {
    token.len() > 2
        && token.starts_with('{')
        && token.ends_with('}')
        && token[1..token.len() - 1]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn push_token(raw: &str, start: usize, end: usize, out: &mut Vec<Tok>) {
    let lower = raw.to_lowercase();
    if is_slot_token(&lower) {
        out.push(Tok {
            word: lower,
            start,
            end,
        });
    } else {
        let cleaned: String = lower.chars().filter(|&c| c != '{' && c != '}').collect();
        if !cleaned.is_empty() {
            out.push(Tok {
                word: cleaned,
                start,
                end,
            });
        }
    }
}

fn tokenize(text: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (i, c) in text.char_indices() {
        if is_word_char(c) {
            if start.is_none() {
                start = Some(i);
            }
        } else if let Some(s) = start.take() {
            push_token(&text[s..i], s, i, &mut out);
        }
    }
    if let Some(s) = start.take() {
        push_token(&text[s..], s, text.len(), &mut out);
    }
    out
}

/// Interned vocabulary plus the precomputed substitution-cost table.
///
/// Utterance words own ids `0..ucount`; `costs[u * total + v]` is the
/// character-level normalized edit distance between utterance word `u` and
/// vocabulary word `v`.
#[derive(Default)]
struct Vocab {
    map: HashMap<String, u32>,
    words: Vec<String>,
    costs: Vec<f32>,
}

impl Vocab {
    fn intern(&mut self, word: &str) -> u32 {
        if let Some(&id) = self.map.get(word) {
            return id;
        }
        let id = self.words.len() as u32;
        self.words.push(word.to_string());
        self.map.insert(word.to_string(), id);
        id
    }

    fn build_costs(&mut self, ucount: usize) {
        let total = self.words.len();
        let mut costs = Vec::with_capacity(ucount * total);
        for u in 0..ucount {
            for w in 0..total {
                costs.push(char_norm_dist(&self.words[u], &self.words[w]));
            }
        }
        self.costs = costs;
    }

    fn cost(&self, uid: u32, vid: u32) -> f32 {
        self.costs[uid as usize * self.words.len() + vid as usize]
    }
}

fn value_word_idx(
    raw: &str,
    vocab: &mut Vocab,
    cache: &mut HashMap<String, (Rc<[u32]>, usize)>,
    uniq: &mut Vec<Rc<[u32]>>,
) -> (Rc<[u32]>, usize) {
    if let Some(entry) = cache.get(raw) {
        return entry.clone();
    }
    let ids: Rc<[u32]> = normalize(&raw.replace('-', " "))
        .iter()
        .map(|w| vocab.intern(w))
        .collect::<Vec<u32>>()
        .into();
    let idx = uniq.len();
    uniq.push(Rc::clone(&ids));
    cache.insert(raw.to_string(), (Rc::clone(&ids), idx));
    (ids, idx)
}

/// Character-level edit distance divided by the longer length; 0 is identical.
fn char_norm_dist(a: &str, b: &str) -> f32 {
    if a == b {
        return 0.0;
    }
    if a.is_empty() || b.is_empty() {
        return 1.0;
    }
    let (dist, denom) = if a.is_ascii() && b.is_ascii() {
        let (d, n) = (
            edit_distance(a.as_bytes(), b.as_bytes()),
            a.len().max(b.len()),
        );
        (d, n)
    } else {
        let ac: Vec<char> = a.chars().collect();
        let bc: Vec<char> = b.chars().collect();
        (edit_distance(&ac, &bc), ac.len().max(bc.len()))
    };
    dist as f32 / denom as f32
}

/// Plain Levenshtein distance over slices. Short rows use stack buffers.
fn edit_distance<T: PartialEq>(a: &[T], b: &[T]) -> usize {
    const STACK: usize = 64;
    let (long, short) = if a.len() >= b.len() { (a, b) } else { (b, a) };
    if short.len() < STACK {
        let mut prev = [0u32; STACK];
        let mut curr = [0u32; STACK];
        let width = short.len() + 1;
        finish_distance(long, short, &mut prev[..width], &mut curr[..width])
    } else {
        let mut prev: Vec<u32> = (0..=short.len() as u32).collect();
        let mut curr = vec![0u32; short.len() + 1];
        finish_distance(long, short, &mut prev, &mut curr)
    }
}

fn finish_distance<T: PartialEq>(
    long: &[T],
    short: &[T],
    prev: &mut [u32],
    curr: &mut [u32],
) -> usize {
    for (j, _) in short.iter().enumerate() {
        prev[j + 1] = j as u32 + 1;
    }
    for (i, x) in long.iter().enumerate() {
        curr[0] = i as u32 + 1;
        for (j, y) in short.iter().enumerate() {
            let sub = u32::from(x != y);
            curr[j + 1] = (prev[j] + sub).min(prev[j + 1] + 1).min(curr[j] + 1);
        }
        prev.copy_from_slice(curr);
    }
    prev[short.len()] as usize
}

/// A normalized phrase template.
enum Pattern {
    /// Literal word (interned vocabulary id).
    Lit(u32),
    /// Fixed-list slot with its candidate values (shared across phrases
    /// that reuse the same value list).
    Fixed {
        name: String,
        values: Rc<Vec<FixedValue>>,
    },
    /// Free-text slot; only valid as the last token.
    Free { name: String },
    /// Slot with no list or declaration; never matches well.
    Unknown { name: String },
}

struct FixedValue {
    /// Canonical value reported in `Match::slots`, e.g. `right-half`.
    canonical: String,
    /// Display form used when rendering, e.g. `right half`.
    display: String,
    /// Normalized value words (interned ids).
    ids: Rc<[u32]>,
    /// Index into the call-global distinct-value list.
    uidx: usize,
}

/// Precomputed `word_dist` for every utterance span against every distinct
/// fixed-list value. `None` when values barely repeat (precomputation would
/// cost more than it saves); callers then fall back to direct computation.
struct SpanTable {
    /// `(utterance words + 1)`; spans are indexed `[s * stride + i]`.
    stride: usize,
    /// Number of distinct values.
    count: usize,
    costs: Vec<f32>,
}

impl SpanTable {
    fn build(
        uids: &[u32],
        uniq: &[Rc<[u32]>],
        vocab: &Vocab,
        scratch: &mut Vec<f32>,
        occurrences: usize,
    ) -> Option<SpanTable> {
        let n = uids.len();
        let stride = n + 1;
        let count = uniq.len();
        // Precompute only for a clear repeat win within a memory cap.
        if count == 0 || 2 * count >= occurrences.max(1) || stride * stride * count > 1_000_000 {
            return None;
        }
        let mut costs = vec![0.0; stride * stride * count];
        for s in 0..=n {
            for i in s..=n {
                for (vi, ids) in uniq.iter().enumerate() {
                    costs[(s * stride + i) * count + vi] =
                        word_dist(&uids[s..i], ids, vocab, scratch);
                }
            }
        }
        Some(SpanTable {
            stride,
            count,
            costs,
        })
    }

    fn get(&self, s: usize, i: usize, vi: usize) -> f32 {
        self.costs[(s * self.stride + i) * self.count + vi]
    }
}

struct Prepared<'a> {
    command_id: &'a str,
    template: &'a str,
    tmpl: Vec<Pattern>,
}

/// Backpointer for one alignment cell.
#[derive(Debug, Clone, Copy)]
enum Back {
    /// Start cell.
    Zero,
    /// Align utterance word with template word (literal or unknown slot).
    Sub,
    /// Skip a template token.
    Ins,
    /// Skip an utterance word.
    Del,
    /// Align utterance span `..prev` with fixed-list value `val`.
    Slot { prev: usize, val: usize },
    /// Capture utterance span `prev..` for a free slot.
    Free { prev: usize },
}

/// Reusable scratch buffers shared across candidates.
#[derive(Default)]
struct Ctx {
    dp: Vec<f32>,
    back: Vec<Back>,
    word: Vec<f32>,
    fills: Vec<CompactFill>,
}

struct Scored {
    distance: f32,
    idx: usize,
}

/// Allocation-free record of one filled slot: template index plus, for
/// fixed slots, the winning value index, or, for free slots, the captured
/// utterance span.
#[derive(Clone, Copy)]
struct CompactFill {
    tok: usize,
    val: usize,
    prev: usize,
    end: usize,
}

/// Pick the cheapest of (diagonal, delete, insert), preferring diagonal ties.
fn pick(c0: f32, b0: Back, c1: f32, b1: Back, c2: f32, b2: Back) -> (f32, Back) {
    if c0 <= c1 && c0 <= c2 {
        (c0, b0)
    } else if c1 <= c2 {
        (c1, b1)
    } else {
        (c2, b2)
    }
}

/// Word-level edit distance between utterance span `span` (vocab ids) and
/// value words `vids`, using the precomputed substitution table. Reuses
/// `scratch` across calls so the inner loop does not allocate.
fn word_dist(span: &[u32], vids: &[u32], vocab: &Vocab, scratch: &mut Vec<f32>) -> f32 {
    let (n, m) = (span.len(), vids.len());
    if n == 0 {
        return m as f32;
    }
    if m == 0 {
        return n as f32;
    }
    let stride = m + 1;
    let need = (n + 1) * stride;
    if scratch.len() < need {
        scratch.resize(need, 0.0);
    }
    scratch[0] = 0.0;
    for (j, cell) in scratch.iter_mut().enumerate().take(stride).skip(1) {
        *cell = j as f32;
    }
    for i in 1..=n {
        scratch[i * stride] = i as f32;
        for j in 1..=m {
            let sub = vocab.cost(span[i - 1], vids[j - 1]);
            let diag = scratch[(i - 1) * stride + (j - 1)] + sub;
            let del = scratch[(i - 1) * stride + j] + 1.0;
            let ins = scratch[i * stride + (j - 1)] + 1.0;
            scratch[i * stride + j] = diag.min(del).min(ins);
        }
    }
    scratch[n * stride + m]
}

/// Align utterance words `uids` with template `tmpl`, returning the raw
/// cost. Compact fills are left in `ctx.fills` (template order) for the
/// caller to count or render.
fn align(
    utt: &[Tok],
    uids: &[u32],
    tmpl: &[Pattern],
    vocab: &Vocab,
    spans: Option<&SpanTable>,
    ctx: &mut Ctx,
) -> f32 {
    let n = utt.len();
    let m = tmpl.len();
    let stride = m + 1;
    let at = |i: usize, j: usize| i * stride + j;

    // Grow-only buffers: every cell below is overwritten before it is read.
    if ctx.dp.len() < (n + 1) * stride {
        ctx.dp.resize((n + 1) * stride, 0.0);
    }
    if ctx.back.len() < (n + 1) * stride {
        ctx.back.resize((n + 1) * stride, Back::Zero);
    }
    ctx.dp[at(0, 0)] = 0.0;
    ctx.back[at(0, 0)] = Back::Zero;

    for i in 1..=n {
        ctx.dp[at(i, 0)] = i as f32;
        ctx.back[at(i, 0)] = Back::Del;
    }
    for j in 1..=m {
        let (cost, back) = match &tmpl[j - 1] {
            Pattern::Lit(_) | Pattern::Unknown { .. } => (ctx.dp[at(0, j - 1)] + 1.0, Back::Ins),
            Pattern::Fixed { values, .. } => {
                let mut shortest = usize::MAX;
                let mut shortest_val = 0;
                for (vi, v) in values.iter().enumerate() {
                    if v.ids.len() < shortest {
                        shortest = v.ids.len();
                        shortest_val = vi;
                    }
                }
                let ins = if shortest == usize::MAX {
                    1.0
                } else {
                    shortest as f32
                };
                (
                    ctx.dp[at(0, j - 1)] + ins,
                    Back::Slot {
                        prev: 0,
                        val: shortest_val,
                    },
                )
            }
            Pattern::Free { .. } if j == m => (f32::INFINITY, Back::Free { prev: 0 }),
            Pattern::Free { .. } => (ctx.dp[at(0, j - 1)] + 1.0, Back::Ins),
        };
        ctx.dp[at(0, j)] = cost;
        ctx.back[at(0, j)] = back;
    }

    for i in 1..=n {
        for j in 1..=m {
            let (cost, back) = match &tmpl[j - 1] {
                Pattern::Lit(lit) => {
                    let sub = vocab.cost(uids[i - 1], *lit);
                    pick(
                        ctx.dp[at(i - 1, j - 1)] + sub,
                        Back::Sub,
                        ctx.dp[at(i - 1, j)] + 1.0,
                        Back::Del,
                        ctx.dp[at(i, j - 1)] + 1.0,
                        Back::Ins,
                    )
                }
                Pattern::Unknown { .. } => pick(
                    ctx.dp[at(i - 1, j - 1)] + 1.0,
                    Back::Sub,
                    ctx.dp[at(i - 1, j)] + 1.0,
                    Back::Del,
                    ctx.dp[at(i, j - 1)] + 1.0,
                    Back::Ins,
                ),
                Pattern::Fixed { values, .. } => {
                    let mut best_c = ctx.dp[at(i - 1, j)] + 1.0;
                    let mut best_b = Back::Del;
                    if let Some(table) = spans {
                        for s in 0..=i {
                            let base = ctx.dp[at(s, j - 1)];
                            if base.is_infinite() {
                                continue;
                            }
                            let width = (i - s) as f32;
                            for (vi, v) in values.iter().enumerate() {
                                // Lower bound: each word of length difference costs >= 1.
                                if base + (width - v.ids.len() as f32).abs() >= best_c {
                                    continue;
                                }
                                let c = base + table.get(s, i, v.uidx);
                                if c < best_c {
                                    best_c = c;
                                    best_b = Back::Slot { prev: s, val: vi };
                                }
                            }
                        }
                    } else {
                        for s in 0..=i {
                            let base = ctx.dp[at(s, j - 1)];
                            if base.is_infinite() {
                                continue;
                            }
                            let width = (i - s) as f32;
                            for (vi, v) in values.iter().enumerate() {
                                // Lower bound: each word of length difference costs >= 1.
                                if base + (width - v.ids.len() as f32).abs() >= best_c {
                                    continue;
                                }
                                let c = base + word_dist(&uids[s..i], &v.ids, vocab, &mut ctx.word);
                                if c < best_c {
                                    best_c = c;
                                    best_b = Back::Slot { prev: s, val: vi };
                                }
                            }
                        }
                    }
                    if best_c.is_infinite() {
                        (ctx.dp[at(i, j - 1)] + 1.0, Back::Ins)
                    } else {
                        (best_c, best_b)
                    }
                }
                Pattern::Free { .. } if j == m => {
                    // Capture all remaining words; must capture at least one.
                    let mut best_c = f32::INFINITY;
                    let mut best_s = 0;
                    for s in 0..i {
                        let c = ctx.dp[at(s, j - 1)];
                        if c < best_c {
                            best_c = c;
                            best_s = s;
                        }
                    }
                    (best_c, Back::Free { prev: best_s })
                }
                Pattern::Free { .. } => pick(
                    ctx.dp[at(i - 1, j - 1)],
                    Back::Free { prev: i - 1 },
                    ctx.dp[at(i - 1, j)] + 1.0,
                    Back::Del,
                    ctx.dp[at(i, j - 1)] + 1.0,
                    Back::Ins,
                ),
            };
            ctx.dp[at(i, j)] = cost;
            ctx.back[at(i, j)] = back;
        }
    }

    ctx.fills.clear();
    let mut fills = std::mem::take(&mut ctx.fills);
    let (mut i, mut j) = (n, m);
    while i > 0 || j > 0 {
        match ctx.back[at(i, j)] {
            Back::Zero => break,
            Back::Sub => {
                i -= 1;
                j -= 1;
            }
            Back::Del => {
                i -= 1;
            }
            Back::Ins => {
                j -= 1;
            }
            Back::Slot { prev, val } => {
                fills.push(CompactFill {
                    tok: j - 1,
                    val,
                    prev,
                    end: i,
                });
                i = prev;
                j -= 1;
            }
            Back::Free { prev } => {
                fills.push(CompactFill {
                    tok: j - 1,
                    val: 0,
                    prev,
                    end: i,
                });
                i = prev;
                j -= 1;
            }
        }
    }
    fills.reverse();
    let cost = ctx.dp[at(n, m)];
    ctx.fills = fills;
    cost
}

/// Latest compact fill for template token `tok`, if any.
fn fill_for(fills: &[CompactFill], tok: usize) -> Option<&CompactFill> {
    fills.iter().rev().find(|f| f.tok == tok)
}

/// Verbatim captured text for a free-slot fill.
fn captured<'a>(utt: &[Tok], raw: &'a str, prev: usize, end: usize) -> &'a str {
    if prev < end && end <= utt.len() {
        raw[utt[prev].start..utt[end - 1].end].trim()
    } else {
        ""
    }
}

/// Exact word count of the phrase rendered from `fills`. Fixed slots
/// contribute their display form, free slots their captured text, so this
/// matches `render` exactly.
fn count_words(utt: &[Tok], raw: &str, tmpl: &[Pattern], fills: &[CompactFill]) -> usize {
    tmpl.iter()
        .enumerate()
        .map(|(t, p)| match p {
            Pattern::Lit(_) | Pattern::Unknown { .. } => 1,
            Pattern::Fixed { values, .. } => match fill_for(fills, t) {
                Some(f) => values
                    .get(f.val)
                    .map(|v| v.display.split_whitespace().count())
                    .unwrap_or(1),
                None => 1,
            },
            Pattern::Free { .. } => match fill_for(fills, t) {
                Some(f) => {
                    let n = captured(utt, raw, f.prev, f.end).split_whitespace().count();
                    if n == 0 {
                        1
                    } else {
                        n
                    }
                }
                None => 1,
            },
        })
        .sum()
}

/// Build the slot map and the rendered phrase from compact fills.
fn render(
    utt: &[Tok],
    raw: &str,
    tmpl: &[Pattern],
    fills: &[CompactFill],
    vocab: &Vocab,
) -> (BTreeMap<String, String>, String) {
    let mut slots = BTreeMap::new();
    let mut parts: Vec<String> = Vec::with_capacity(tmpl.len());
    for (t, p) in tmpl.iter().enumerate() {
        match p {
            Pattern::Lit(id) => parts.push(vocab.words[*id as usize].clone()),
            Pattern::Unknown { name } => parts.push(format!("{{{name}}}")),
            Pattern::Fixed { name, values } => match fill_for(fills, t) {
                Some(f) => match values.get(f.val) {
                    Some(v) => {
                        slots.insert(name.clone(), v.canonical.clone());
                        parts.push(v.display.clone());
                    }
                    None => parts.push(format!("{{{name}}}")),
                },
                None => parts.push(format!("{{{name}}}")),
            },
            Pattern::Free { name } => match fill_for(fills, t) {
                Some(f) => {
                    let text = captured(utt, raw, f.prev, f.end);
                    if text.is_empty() {
                        parts.push(format!("{{{name}}}"));
                    } else {
                        slots.insert(name.clone(), text.to_string());
                        parts.push(text.to_string());
                    }
                }
                None => parts.push(format!("{{{name}}}")),
            },
        }
    }
    (slots, parts.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::time::{Duration, Instant};

    #[derive(Default)]
    struct TestSlots {
        lists: HashMap<(String, String), Vec<String>>,
        free: HashSet<(String, String)>,
    }

    impl TestSlots {
        fn with_list(mut self, cmd: &str, slot: &str, values: &[&str]) -> Self {
            self.lists.insert(
                (cmd.to_string(), slot.to_string()),
                values.iter().map(|s| s.to_string()).collect(),
            );
            self
        }

        fn with_free(mut self, cmd: &str, slot: &str) -> Self {
            self.free.insert((cmd.to_string(), slot.to_string()));
            self
        }

        fn std() -> Self {
            Self::default()
                .with_list("focus", "window", &["terminal", "editor", "browser"])
                .with_list("move", "window", &["terminal", "editor", "browser"])
                .with_list(
                    "move",
                    "place",
                    &[
                        "left-half",
                        "right-half",
                        "top-half",
                        "bottom-half",
                        "full",
                        "center",
                    ],
                )
                .with_free("note", "name")
                .with_list("project", "project", &["jisuno", "inkuno", "rex"])
        }
    }

    impl SlotSource for TestSlots {
        fn values(&self, command_id: &str, slot: &str) -> Option<SlotValues<'_>> {
            let key = (command_id.to_string(), slot.to_string());
            if let Some(v) = self.lists.get(&key) {
                return Some(SlotValues::List(v));
            }
            if self.free.contains(&key) {
                return Some(SlotValues::Free);
            }
            None
        }
    }

    fn cand<'a>(id: &'a str, phrase: &'a str) -> Candidate<'a> {
        Candidate {
            command_id: id,
            phrase,
            when_apps: None,
        }
    }

    #[test]
    fn normalize_lowercase_strips_punctuation() {
        assert_eq!(normalize("Start working."), vec!["start", "working"]);
        assert_eq!(
            normalize("move {window} to {place}"),
            vec!["move", "{window}", "to", "{place}"]
        );
        assert_eq!(normalize("right-half"), vec!["right", "half"]);
        assert!(normalize("... !!! ").is_empty());
    }

    #[test]
    fn exact_match_runs() {
        let slots = TestSlots::std();
        let cands = [cand("settings", "open settings")];
        match match_utterance("open settings", None, &cands, &slots) {
            Outcome::Run(m) => {
                assert_eq!(m.command_id, "settings");
                assert_eq!(m.phrase, "open settings");
                assert!(m.slots.is_empty());
                assert_eq!(m.distance, 0.0);
            }
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn punctuation_and_casing_match() {
        let slots = TestSlots::std();
        let cands = [cand("work.start", "start working")];
        match match_utterance("Start working.", None, &cands, &slots) {
            Outcome::Run(m) => assert_eq!(m.command_id, "work.start"),
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn minor_mishearing_accepted() {
        let slots = TestSlots::std();
        let cands = [cand("focus", "focus {window}")];
        match match_utterance("focus editr", None, &cands, &slots) {
            Outcome::Run(m) => {
                assert_eq!(m.command_id, "focus");
                assert_eq!(m.slots.get("window").map(String::as_str), Some("editor"));
            }
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn too_far_suggests() {
        let slots = TestSlots::std();
        let cands = [
            cand("settings", "open settings"),
            cand("work.start", "start working"),
        ];
        match match_utterance("cancel everything right now", None, &cands, &slots) {
            Outcome::DidYouMean { phrase, command_id } => {
                assert!(!phrase.is_empty());
                assert!(!command_id.is_empty());
            }
            other => panic!("expected DidYouMean, got {other:?}"),
        }
    }

    #[test]
    fn near_tie_suggests() {
        let slots = TestSlots::std();
        // Literal and slot template collide exactly on this utterance.
        let cands = [
            cand("focus-lit", "focus editor"),
            cand("focus", "focus {window}"),
        ];
        match match_utterance("focus editor", None, &cands, &slots) {
            Outcome::DidYouMean { phrase, .. } => assert_eq!(phrase, "focus editor"),
            other => panic!("expected DidYouMean, got {other:?}"),
        }
    }

    #[test]
    fn when_apps_filters_before_matching() {
        let slots = TestSlots::std();
        let mail = ["com.apple.mail".to_string()];
        let cands = [Candidate {
            command_id: "reply",
            phrase: "reply",
            when_apps: Some(&mail),
        }];
        match match_utterance("reply", Some("com.apple.mail"), &cands, &slots) {
            Outcome::Run(m) => assert_eq!(m.command_id, "reply"),
            other => panic!("expected Run, got {other:?}"),
        }
        assert!(matches!(
            match_utterance("reply", Some("com.apple.safari"), &cands, &slots),
            Outcome::NoMatch
        ));
        assert!(matches!(
            match_utterance("reply", None, &cands, &slots),
            Outcome::NoMatch
        ));
    }

    #[test]
    fn fixed_slots_fill_canonical_values() {
        let slots = TestSlots::std();
        let cands = [cand("move", "move {window} to {place}")];
        match match_utterance("move terminal to right half", None, &cands, &slots) {
            Outcome::Run(m) => {
                assert_eq!(m.command_id, "move");
                assert_eq!(m.slots.get("window").map(String::as_str), Some("terminal"));
                assert_eq!(m.slots.get("place").map(String::as_str), Some("right-half"));
                assert_eq!(m.phrase, "move terminal to right half");
            }
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn free_slot_captures_verbatim_tail() {
        let slots = TestSlots::std();
        let cands = [cand("note", "open note {name}")];
        match match_utterance("open note Standup Notes", None, &cands, &slots) {
            Outcome::Run(m) => {
                assert_eq!(m.command_id, "note");
                assert_eq!(
                    m.slots.get("name").map(String::as_str),
                    Some("Standup Notes")
                );
            }
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn declared_fixed_slot_matches() {
        let slots = TestSlots::std();
        let cands = [cand("project", "open project {project}")];
        match match_utterance("open project inkuno", None, &cands, &slots) {
            Outcome::Run(m) => {
                assert_eq!(m.command_id, "project");
                assert_eq!(m.slots.get("project").map(String::as_str), Some("inkuno"));
            }
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn empty_utterance_is_no_match() {
        let slots = TestSlots::std();
        let cands = [cand("settings", "open settings")];
        assert!(matches!(
            match_utterance("", None, &cands, &slots),
            Outcome::NoMatch
        ));
        assert!(matches!(
            match_utterance("  ...  ", None, &cands, &slots),
            Outcome::NoMatch
        ));
    }

    #[test]
    fn no_candidates_is_no_match() {
        let slots = TestSlots::std();
        assert!(matches!(
            match_utterance("open settings", None, &[], &slots),
            Outcome::NoMatch
        ));
    }

    /// 200 commands x 3 phrases with 30-value slot lists must score fast.
    /// Ignored on debug builds; enforced with release optimizations.
    #[test]
    #[cfg_attr(debug_assertions, ignore)]
    fn perf_large_config_under_5ms() {
        let value_list: Vec<String> = (0..30).map(|i| format!("value-{i}")).collect();
        struct AllList<'a> {
            values: &'a [String],
        }
        impl SlotSource for AllList<'_> {
            fn values(&self, _command_id: &str, _slot: &str) -> Option<SlotValues<'_>> {
                Some(SlotValues::List(self.values))
            }
        }
        let slots = AllList {
            values: &value_list,
        };

        let ids: Vec<String> = (0..200).map(|c| format!("cmd{c}")).collect();
        let mut phrases: Vec<String> = Vec::with_capacity(600);
        for c in 0..200 {
            for p in 0..3 {
                phrases.push(format!("cmd{c} action{p} {{item}}"));
            }
        }
        let mut cands: Vec<Candidate> = Vec::with_capacity(600);
        for c in 0..200 {
            for p in 0..3 {
                cands.push(Candidate {
                    command_id: ids[c].as_str(),
                    phrase: phrases[c * 3 + p].as_str(),
                    when_apps: None,
                });
            }
        }
        assert_eq!(cands.len(), 600);

        let start = Instant::now();
        let outcome = match_utterance("cmd42 action1 value 7", None, &cands, &slots);
        let elapsed = start.elapsed();
        println!("large-config match took {elapsed:?}: {outcome:?}");
        assert!(
            elapsed < Duration::from_millis(5),
            "matcher took {elapsed:?}, budget is 5 ms"
        );
    }
}
