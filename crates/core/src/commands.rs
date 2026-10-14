//! Diktierbefehle ("Punkt", "neue Zeile", ...) -> Satzzeichen und Formatierung.
//!
//! Der [`Processor`] ist zustandsbehaftet, damit mehrere nacheinander
//! transkribierte Segmente nahtlos zusammengesetzt werden (Leerzeichen,
//! Großschreibung nach Satzende).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    De,
    En,
    /// Befehle beider Sprachen aktiv (Whisper-Spracherkennung auf "auto").
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    /// Zeichen direkt an das vorige Wort hängen: `.` `,` `?`
    Attach(&'static str),
    /// Zeichen mit Leerzeichen davor, ohne Leerzeichen danach: `(` `„`
    Open(&'static str),
    /// Zeichen mit Leerzeichen danach, direkt am vorigen Wort: `)` `“`
    Close(&'static str),
    Newline(usize),
}

struct Cmd {
    phrase: &'static [&'static str],
    action: Action,
    /// Kann auch ein normales Wort sein ("am Punkt", "Punkt 3").
    ambiguous: bool,
}

const fn c(phrase: &'static [&'static str], action: Action, ambiguous: bool) -> Cmd {
    Cmd { phrase, action, ambiguous }
}

use Action::*;

static DE: &[Cmd] = &[
    c(&["punkt"], Attach("."), true),
    c(&["komma"], Attach(","), true),
    c(&["fragezeichen"], Attach("?"), false),
    c(&["ausrufezeichen"], Attach("!"), false),
    c(&["doppelpunkt"], Attach(":"), false),
    c(&["semikolon"], Attach(";"), false),
    c(&["strichpunkt"], Attach(";"), false),
    c(&["klammer", "auf"], Open("("), false),
    c(&["klammer", "zu"], Close(")"), false),
    c(&["anführungszeichen", "auf"], Open("„"), false),
    c(&["anführungszeichen", "zu"], Close("“"), false),
    c(&["anführungszeichen", "unten"], Open("„"), false),
    c(&["anführungszeichen", "oben"], Close("“"), false),
    c(&["neue", "zeile"], Newline(1), false),
    c(&["neuer", "absatz"], Newline(2), false),
    c(&["absatzzeichen"], Open("§"), false),
    c(&["paragraf"], Open("§"), true),
    c(&["paragraph"], Open("§"), true),
];

static EN: &[Cmd] = &[
    c(&["period"], Attach("."), true),
    c(&["full", "stop"], Attach("."), false),
    c(&["comma"], Attach(","), true),
    c(&["question", "mark"], Attach("?"), false),
    c(&["exclamation", "mark"], Attach("!"), false),
    c(&["exclamation", "point"], Attach("!"), false),
    c(&["colon"], Attach(":"), true),
    c(&["semicolon"], Attach(";"), false),
    c(&["open", "bracket"], Open("("), false),
    c(&["close", "bracket"], Close(")"), false),
    c(&["open", "quote"], Open("“"), false),
    c(&["close", "quote"], Close("”"), false),
    c(&["new", "line"], Newline(1), false),
    c(&["new", "paragraph"], Newline(2), false),
];

/// Wörter, nach denen ein mehrdeutiger Befehl ein normales Wort ist.
static NOT_AFTER: &[&str] = &[
    // de
    "am", "ein", "einen", "einem", "einer", "der", "die", "das", "dem", "den", "des", "dieser",
    "diesem", "diesen", "dieses", "zum", "zu", "kein", "keinen", "jeden", "jeder", "springender",
    "wunder", "toter", "strittige", "strittigen", "kritischen", "kritische", "genau", "auf",
    // en
    "a", "the", "this", "that", "of", "in", "time", "trial", "grace", "same", "that's", "at",
];

#[derive(Debug, Clone)]
pub struct Processor {
    lang: Lang,
    commands_enabled: bool,
    /// Letztes ausgegebenes Zeichen verbietet ein Leerzeichen (Zeilenanfang, `(`, `„`).
    no_space: bool,
    /// Nächstes Wort groß schreiben.
    cap_next: bool,
    started: bool,
}

impl Processor {
    pub fn new(lang: Lang) -> Self {
        Self {
            lang,
            commands_enabled: true,
            no_space: true,
            cap_next: false,
            started: false,
        }
    }

    pub fn set_commands_enabled(&mut self, on: bool) {
        self.commands_enabled = on;
    }

    /// Setzt den Zustand zurück (z. B. neues Diktat).
    pub fn reset(&mut self) {
        *self = Self { lang: self.lang, commands_enabled: self.commands_enabled, ..Self::new(self.lang) };
    }

    fn table(&self) -> Vec<&'static Cmd> {
        let mut v: Vec<&'static Cmd> = Vec::new();
        if matches!(self.lang, Lang::De | Lang::Auto) {
            v.extend(DE.iter());
        }
        if matches!(self.lang, Lang::En | Lang::Auto) {
            v.extend(EN.iter());
        }
        v
    }

    /// Verarbeitet ein Whisper-Segment und liefert den einzufügenden Text,
    /// inklusive führendem Leerzeichen, falls es an vorherigen Text anschließt.
    pub fn feed(&mut self, text: &str) -> String {
        let tokens = split_glued(text);
        let norm: Vec<String> = tokens.iter().map(|t| normalize(t)).collect();
        let table = self.table();
        let mut out = String::new();

        let mut i = 0;
        while i < tokens.len() {
            let found = if self.commands_enabled {
                find_command(&table, &norm[i..])
            } else {
                None
            };
            if let Some((cmd, n)) = found {
                let ambiguous_blocked = cmd.ambiguous && {
                    let prev_blocks = i > 0 && NOT_AFTER.contains(&norm[i - 1].as_str());
                    let next_is_number = norm
                        .get(i + n)
                        .is_some_and(|w| w.chars().next().is_some_and(|ch| ch.is_ascii_digit()));
                    prev_blocks || next_is_number
                };
                if !ambiguous_blocked {
                    self.apply(cmd.action, &mut out);
                    i += n;
                    continue;
                }
            }
            self.word(&tokens[i], &mut out);
            i += 1;
        }
        out
    }

    fn lead_space(&self, out: &str) -> bool {
        // Am Segmentanfang entscheidet der Zustand, sonst der bisherige Ausgabetext.
        if out.is_empty() {
            self.started && !self.no_space
        } else {
            !self.no_space
        }
    }

    fn word(&mut self, tok: &str, out: &mut String) {
        if self.lead_space(out) {
            out.push(' ');
        }
        let mut w = tok.to_string();
        if self.cap_next {
            w = capitalize(&w);
            self.cap_next = false;
        }
        out.push_str(&w);
        self.started = true;
        self.no_space = false;
        if w.ends_with(['.', '?', '!']) {
            self.cap_next = true;
        }
    }

    fn apply(&mut self, a: Action, out: &mut String) {
        match a {
            Attach(s) | Close(s) => {
                // Whisper-Eigeninterpunktion vor dem Befehl entfernen (", Komma" -> ",").
                if matches!(a, Attach(_)) {
                    while out.ends_with(['.', ',', ';', ':', '!', '?']) {
                        out.pop();
                    }
                }
                out.push_str(s);
                self.started = true;
                self.no_space = false;
                if matches!(s, "." | "?" | "!") {
                    self.cap_next = true;
                }
            }
            Open(s) => {
                if self.lead_space(out) {
                    out.push(' ');
                }
                out.push_str(s);
                self.started = true;
                // "§" bekommt ein Leerzeichen danach, Klammern/Anführung nicht.
                self.no_space = s != "§";
            }
            Newline(n) => {
                while out.ends_with(' ') {
                    out.pop();
                }
                for _ in 0..n {
                    out.push('\n');
                }
                self.started = true;
                self.no_space = true;
                self.cap_next = true;
            }
        }
    }
}

/// Zusammensetzungen auf "-punkt", die echte Wörter sind (nicht aufspalten).
static PUNKT_COMPOUNDS: &[&str] = &[
    "stand", "schwer", "zeit", "gesichts", "mittel", "ausgangs", "treff", "höhe", "wende", "brenn",
    "knoten", "doppel", "kern", "streit", "haupt", "gefrier", "siede", "tief", "anhalts", "angriffs",
    "blick", "betreff", "fix", "end", "start", "ziel", "punkt", "dreh", "kontroll", "sammel",
    "haltepunkt", "wunde", "bezugs", "kritik", "verkaufs", "zahlungs", "ansatz", "vor", "tat",
];

/// Einwort-Befehle, die per Bindestrich an Nachbarwörter geraten können.
static HYPHEN_CMDS: &[&str] =
    &["komma", "punkt", "fragezeichen", "ausrufezeichen", "doppelpunkt", "semikolon"];

/// Whisper klebt gesprochene Befehle manchmal ans vorige Wort ("Schreibenpunkt",
/// "Müllerkomma"). "komma" wird immer, "punkt" nur am Textende und nicht in
/// bekannten Zusammensetzungen abgespalten.
fn split_glued(text: &str) -> Vec<String> {
    // Whisper verbindet Befehle gern per Bindestrich: "Komma-Damen", "Herren-Komma!".
    let words: Vec<&str> = text
        .split_whitespace()
        .flat_map(|w| {
            let parts: Vec<&str> = w.split('-').collect();
            let has_cmd = parts.len() > 1
                && parts.iter().any(|p| HYPHEN_CMDS.contains(&normalize(p).as_str()));
            if has_cmd { parts } else { vec![w] }
        })
        .collect();
    let mut out = Vec::with_capacity(words.len());
    for (idx, w) in words.iter().enumerate() {
        let core = w.trim_end_matches(['.', ',', ';', ':', '!', '?']);
        let trailing = &w[core.len()..];
        let lower = core.to_lowercase();
        let is_last = idx + 1 == words.len();
        let suffix = if lower.ends_with("komma") {
            Some("komma")
        } else if is_last && lower.ends_with("punkt") {
            Some("punkt")
        } else {
            None
        };
        if let Some(suf) = suffix {
            let cut = core.len() - suf.len();
            if cut >= 3 && core.is_char_boundary(cut) {
                let prefix = &core[..cut];
                let compound = suf == "punkt" && PUNKT_COMPOUNDS.contains(&prefix.to_lowercase().as_str());
                if !compound {
                    out.push(prefix.to_string());
                    out.push(format!("{}{}", &core[cut..], trailing));
                    continue;
                }
            }
        }
        out.push(w.to_string());
    }
    out
}

fn normalize(tok: &str) -> String {
    tok.trim_matches(|ch: char| matches!(ch, '.' | ',' | ';' | ':' | '!' | '?' | '"' | '„' | '“' | '”'))
        .to_lowercase()
}

/// Längste passende Phrase ab Position `i`.
fn find_command<'a>(table: &[&'a Cmd], rest: &[String]) -> Option<(&'a Cmd, usize)> {
    table
        .iter()
        .filter(|cmd| {
            cmd.phrase.len() <= rest.len()
                && cmd.phrase.iter().zip(rest).all(|(p, w)| *p == w.as_str())
        })
        .max_by_key(|cmd| cmd.phrase.len())
        .map(|cmd| (*cmd, cmd.phrase.len()))
}

fn capitalize(s: &str) -> String {
    let mut ch = s.chars();
    match ch.next() {
        Some(f) => f.to_uppercase().collect::<String>() + ch.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(lang: Lang, segs: &[&str]) -> String {
        let mut p = Processor::new(lang);
        segs.iter().map(|s| p.feed(s)).collect()
    }

    #[test]
    fn de_basic() {
        assert_eq!(
            run(Lang::De, &["Sehr geehrter Herr Müller Komma ich danke Ihnen Punkt"]),
            "Sehr geehrter Herr Müller, ich danke Ihnen."
        );
    }

    #[test]
    fn whisper_own_punctuation_is_not_doubled() {
        assert_eq!(
            run(Lang::De, &["Guten Tag, Komma, wie geht es Ihnen? Fragezeichen"]),
            "Guten Tag, wie geht es Ihnen?"
        );
        assert_eq!(run(Lang::De, &["Das ist gut. Punkt."]), "Das ist gut.");
    }

    #[test]
    fn capitalizes_after_sentence_end() {
        assert_eq!(run(Lang::De, &["das ist gut Punkt wir sehen uns"]), "das ist gut. Wir sehen uns");
    }

    #[test]
    fn newline_and_paragraph() {
        assert_eq!(
            run(Lang::De, &["Guten Tag Komma neue Zeile hiermit teile ich mit neuer Absatz im Folgenden"]),
            "Guten Tag,\nHiermit teile ich mit\n\nIm Folgenden"
        );
    }

    #[test]
    fn brackets_and_quotes() {
        assert_eq!(
            run(Lang::De, &["der Kläger Klammer auf A Klammer zu sagt Anführungszeichen auf nein Anführungszeichen zu"]),
            "der Kläger (A) sagt „nein“"
        );
    }

    #[test]
    fn ambiguous_words_are_kept() {
        assert_eq!(run(Lang::De, &["siehe Punkt 3 der Anlage"]), "siehe Punkt 3 der Anlage");
        assert_eq!(run(Lang::De, &["am Punkt angekommen"]), "am Punkt angekommen");
    }

    #[test]
    fn segments_join_with_space() {
        assert_eq!(run(Lang::De, &["Hallo Welt Punkt", "Wie geht es"]), "Hallo Welt. Wie geht es");
        assert_eq!(run(Lang::De, &["Hallo Welt", "Punkt"]), "Hallo Welt.");
    }

    #[test]
    fn paragraph_sign() {
        assert_eq!(run(Lang::De, &["nach Paragraf 823 BGB"]), "nach Paragraf 823 BGB");
        assert_eq!(run(Lang::De, &["nach Absatzzeichen 823 BGB"]), "nach § 823 BGB");
    }

    #[test]
    fn glued_commands_are_split() {
        assert_eq!(run(Lang::De, &["Sehr geehrter Herr Müllerkomma ich danke Ihnen"]), "Sehr geehrter Herr Müller, ich danke Ihnen");
        assert_eq!(run(Lang::De, &["ich danke für Ihr Schreibenpunkt."]), "ich danke für Ihr Schreiben.");
    }

    #[test]
    fn hyphenated_commands_are_split() {
        assert_eq!(
            run(Lang::De, &["Guten Tag meine sehr geehrten Komma-Damen und Herren-Komma!"]),
            "Guten Tag meine sehr geehrten, Damen und Herren,"
        );
    }

    #[test]
    fn real_compounds_are_kept() {
        assert_eq!(run(Lang::De, &["das ist unser Standpunkt"]), "das ist unser Standpunkt");
        assert_eq!(run(Lang::De, &["der Schwerpunkt liegt hier."]), "der Schwerpunkt liegt hier.");
    }

    #[test]
    fn english() {
        assert_eq!(
            run(Lang::En, &["Dear Sir comma thank you period new line best regards"]),
            "Dear Sir, thank you.\nBest regards"
        );
    }

    #[test]
    fn auto_mode_handles_both() {
        assert_eq!(run(Lang::Auto, &["Hallo Komma hello period"]), "Hallo, hello.");
    }

    #[test]
    fn commands_can_be_disabled() {
        let mut p = Processor::new(Lang::De);
        p.set_commands_enabled(false);
        assert_eq!(p.feed("sage Punkt und Komma"), "sage Punkt und Komma");
    }
}
