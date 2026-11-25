//! Source's command buffer (Cbuf) for the gameplay console, so retail
//! testscripts and cfg files run unchanged:
//!
//! - lines split on `;` and newlines outside quotes, `//` comments dropped;
//! - the command name ends at whitespace or one of Source's tokenizer break
//!   characters `{ } ( ) ' :` (`script{{...}` is `script {{...}`);
//! - `alias name "commands"` and alias expansion at the front of the buffer
//!   (Cbuf_InsertText);
//! - `wait [frames]`, `test_wait <seconds>` and `test_runframe` stop the
//!   buffer, everything else in front of them runs in the same frame;
//! - `exec <cfg>` and `test_startscript <file>` (testscripts/, .txt or
//!   .vtest, extension optional) insert a file's commands;
//! - a cvar table for the settings retail scripts set (stored, printed back
//!   like retail's `"name" = "value"`).
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
/// The map logic's clock (seconds, f32 bits), set each frame by
/// crate::dev_console. `test_wait` counts game time, so a script pauses
/// with the game and every `ent_fire` queued before a wait has run (map
/// inputs only flush while the game ticks).
pub static GAME_TIME: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
fn game_time() -> f32 {
    f32::from_bits(GAME_TIME.load(std::sync::atomic::Ordering::Relaxed))
}

/// Commands one frame may run before the rest waits (a runaway alias loop
/// cannot hang the game).
const PER_FRAME: usize = 20_000;

struct Buffer {
    queue: VecDeque<String>,
    aliases: HashMap<String, String>,
    cvars: HashMap<String, String>,
    wait_frames: u32,
    wait_until: Option<f32>,
}
static BUFFER: Mutex<Option<Buffer>> = Mutex::new(None);

fn with<T>(f: impl FnOnce(&mut Buffer) -> T) -> T {
    let mut guard = BUFFER.lock().unwrap();
    f(guard.get_or_insert_with(|| Buffer {
        queue: VecDeque::new(),
        aliases: HashMap::new(),
        cvars: HashMap::new(),
        wait_frames: 0,
        wait_until: None,
    }))
}

/// Split text into commands like Cbuf: `;` and newlines outside quotes end a
/// command, `//` starts a comment to the end of the line.
pub fn split(text: &str) -> Vec<String> {
    let mut out = vec![];
    let mut current = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if !quoted && c == '/' && chars.peek() == Some(&'/') {
            while let Some(&n) = chars.peek() {
                if n == '\n' {
                    break;
                }
                chars.next();
            }
            continue;
        }
        if c == '"' {
            quoted = !quoted;
        }
        if c == '\n' || c == '\r' || (!quoted && c == ';') {
            if c == '\n' {
                quoted = false;
            }
            let t = current.trim();
            if !t.is_empty() {
                out.push(t.to_string());
            }
            current.clear();
            continue;
        }
        current.push(c);
    }
    let t = current.trim();
    if !t.is_empty() {
        out.push(t.to_string());
    }
    out
}

/// The command name and the rest, splitting at whitespace or one of
/// Source's break characters.
pub fn name_and_rest(command: &str) -> (String, &str) {
    let command = command.trim_start();
    if let Some(stripped) = command.strip_prefix('"') {
        let end = stripped.find('"').unwrap_or(stripped.len());
        return (stripped[..end].to_ascii_lowercase(), stripped.get(end + 1..).unwrap_or("").trim_start());
    }
    let end = command
        .find(|c: char| c.is_whitespace() || matches!(c, '{' | '}' | '(' | ')' | '\'' | ':'))
        .unwrap_or(command.len());
    let end = if end == 0 { command.chars().next().map_or(0, char::len_utf8) } else { end };
    (command[..end].to_ascii_lowercase(), command[end..].trim_start())
}

/// Source-style arguments: quoted strings or whitespace-separated words.
pub fn args(rest: &str) -> Vec<String> {
    let mut out = vec![];
    let mut chars = rest.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        let mut word = String::new();
        if c == '"' {
            chars.next();
            for c in chars.by_ref() {
                if c == '"' {
                    break;
                }
                word.push(c);
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() || c == '"' {
                    break;
                }
                word.push(c);
                chars.next();
            }
        }
        out.push(word);
    }
    out
}

/// Queue console input (Cbuf_AddText).
pub fn submit(text: &str) {
    let commands = split(text);
    with(|b| b.queue.extend(commands));
}

/// Cvars retail testscripts and cfgs set; stored so `name` prints them back.
const CVARS: &[(&str, &str)] = &[
    ("sv_cheats", "0"), ("sv_allow_votes", "1"), ("sv_player_stuck_tolerance", "10"),
    ("sv_regeneration_force_on", "0"), ("director_afk_timeout", "45"),
    ("director_no_survivor_bots", "0"), ("mp_gamemode", "coop"),
    ("mp_allowspectators", "1"), ("z_finale_spawn_safety_range", "600"), ("fps_max", "300"),
    ("cl_interp", "0.1"), ("fog_override", "0"), ("fog_start", "-1"), ("fog_end", "-1"),
    ("fog_color", "-1 -1 -1"), ("fog_startskybox", "-1"), ("fog_endskybox", "-1"),
    ("fog_colorskybox", "-1 -1 -1"), ("fog_2dskyboxfogfactor", "1"), ("name", "unnamed"),
];

pub fn cvar(name: &str) -> Option<String> {
    let name = name.to_ascii_lowercase();
    with(|b| b.cvars.get(&name).cloned())
        .or_else(|| CVARS.iter().find(|(n, _)| *n == name).map(|(_, d)| d.to_string()))
}

/// Set or print a table cvar; None when `command` is not one.
pub fn cvar_command(command: &str) -> Option<String> {
    let (name, rest) = name_and_rest(command);
    let default = CVARS.iter().find(|(n, _)| *n == name)?.1;
    let words = args(rest);
    Some(match words.first() {
        None => format!("\"{name}\" = \"{}\" ( def. \"{default}\" )", cvar(&name).unwrap_or_default()),
        Some(_) => {
            let value = words.join(" ");
            with(|b| b.cvars.insert(name, value));
            String::new()
        }
    })
}

/// A cfg file's text (`exec`): `cfg/<name>[.cfg]` under the game folder.
fn cfg_text(root: &Path, name: &str) -> Option<String> {
    let name = name.trim().trim_matches('"');
    let base = root.join("left4dead2/cfg");
    [base.join(name), base.join(format!("{name}.cfg"))]
        .iter()
        .find(|p| p.is_file())
        .and_then(|p| std::fs::read(p).ok())
        .map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// The testscripts folders, in order: the game's, then the rewrite's own.
pub fn testscript_dirs(root: &Path) -> Vec<PathBuf> {
    vec![root.join("left4dead2/testscripts"), PathBuf::from("testscripts")]
}

/// `test_startscript name`: name, name.txt or name.vtest in a testscripts
/// folder.
pub fn testscript_path(root: &Path, name: &str) -> Option<PathBuf> {
    let name = name.trim().trim_matches('"');
    if name.contains("..") {
        return None;
    }
    testscript_dirs(root).into_iter().find_map(|dir| {
        [name.to_string(), format!("{name}.txt"), format!("{name}.vtest")]
            .into_iter()
            .map(|n| dir.join(n))
            .find(|p| p.is_file())
    })
}

/// Commands ready to run this frame, after aliases, waits and file
/// inclusion. Replies from buffer-level commands (alias listing, missing
/// files) come back as `Err` lines for the console.
pub fn take_ready(root: &Path) -> Vec<Result<String, String>> {
    let mut out = vec![];
    with(|b| {
        if let Some(until) = b.wait_until {
            if game_time() < until {
                return;
            }
            b.wait_until = None;
        }
        if b.wait_frames > 0 {
            b.wait_frames -= 1;
            return;
        }
        let mut budget = PER_FRAME;
        while let Some(command) = b.queue.pop_front() {
            if budget == 0 {
                b.queue.push_front(command);
                break;
            }
            budget -= 1;
            let (name, rest) = name_and_rest(&command);
            match name.as_str() {
                "wait" => {
                    b.wait_frames = rest.trim().parse::<u32>().unwrap_or(1).max(1).saturating_sub(1);
                    return;
                }
                "test_runframe" => return,
                "test_wait" => {
                    let seconds = rest.trim().trim_matches('"').parse::<f32>().unwrap_or(0.).clamp(0., 3600.);
                    // At least one tick, so inputs queued before it flush.
                    b.wait_until = Some(game_time() + seconds.max(1. / 30.));
                    return;
                }
                "alias" => {
                    let words = args(rest);
                    match words.as_slice() {
                        [] => out.push(Err(format!(
                            "Current alias commands:\n{}",
                            b.aliases.iter().map(|(k, v)| format!("{k} : {v}")).collect::<Vec<_>>().join("\n")
                        ))),
                        [alias] => {
                            if let Some(v) = b.aliases.get(&alias.to_ascii_lowercase()) {
                                out.push(Err(format!("{alias} : {v}")));
                            }
                        }
                        [alias, body @ ..] => {
                            b.aliases.insert(alias.to_ascii_lowercase(), body.join(" "));
                        }
                    }
                }
                "exec" => match cfg_text(root, rest) {
                    Some(text) => {
                        for c in split(&text).into_iter().rev() {
                            b.queue.push_front(c);
                        }
                    }
                    None => out.push(Err(format!("exec: couldn't exec {}", rest.trim()))),
                },
                "test_startscript" => match testscript_path(root, rest) {
                    Some(path) => {
                        let text = std::fs::read(&path).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
                        for c in split(&text).into_iter().rev() {
                            b.queue.push_front(c);
                        }
                    }
                    None => out.push(Err(format!("Unable to find test script \"{}\"", rest.trim()))),
                },
                _ => {
                    if let Some(body) = b.aliases.get(&name).cloned() {
                        for c in split(&body).into_iter().rev() {
                            b.queue.push_front(c);
                        }
                    } else if name != command.trim_start().split_whitespace().next().unwrap_or("").to_ascii_lowercase() && !rest.is_empty() {
                        // `script{{...}`: a break character ended the name.
                        out.push(Ok(format!("{name} {rest}")));
                    } else {
                        out.push(Ok(command));
                    }
                }
            }
        }
    });
    out
}

/// Commands still queued or waiting (a testscript in progress).
pub fn busy() -> bool {
    with(|b| !b.queue.is_empty() || b.wait_until.is_some() || b.wait_frames > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn splits_like_cbuf_and_tokenizes_break_characters() {
        assert_eq!(split("a 1; b \"x;y\"\n// note\nc // tail"), vec!["a 1", "b \"x;y\"", "c"]);
        assert_eq!(name_and_rest("script{{pr<-null}"), ("script".to_string(), "{{pr<-null}"));
        assert_eq!(name_and_rest("ent_keyvalue 9768024 \"TargetName\" \"a b\""), ("ent_keyvalue".to_string(), "9768024 \"TargetName\" \"a b\""));
        assert_eq!(args("9768024 \"TargetName\" \"a b\" x"), vec!["9768024", "TargetName", "a b", "x"]);
    }
}
