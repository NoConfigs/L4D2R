//! Native text VCD speech/trigger timelines and response follow-ups.
//! Response criteria beyond the concept/survivor are not a complete Director simulation.
use crate::{game_audio::Queue, kv, source_assets::SourceAssets};
use l4d2r_game_core::entities::EntityWorld;
use std::{collections::HashMap, io, path::Path};

#[derive(Clone, Debug)]
struct Response {
    scene: Option<String>,
    /// (target, concept, context "Key:Value,...", delay).
    then: Option<(String, String, String, f32)>,
    delay: f32,
    fire: Option<(String, String, f32)>,
}
#[derive(Clone, Debug)]
enum Action {
    Speak(String),
    Output(usize, String),
    Concept(Option<String>, String),
    Input(String, String),
}
/// A response-rules criterion: `criterion "Name" "Key" "Value" [required]
/// [weight N]`.
#[derive(Clone, Debug)]
struct Criterion {
    key: String,
    value: String,
    required: bool,
    weight: f32,
    /// `criterion "Name" { A B C } required`: matches when any member does.
    any: Vec<(String, String)>,
    name: String,
}
impl Criterion {
    fn test(&self, set: &HashMap<String, String>) -> bool {
        let fact = |k: &str| set.get(k).map_or("", String::as_str);
        if self.any.is_empty() {
            matches(fact(&self.key), &self.value)
        } else {
            self.any.iter().any(|(k, v)| matches(fact(k), v))
        }
    }
}
#[derive(Clone, Debug)]
struct Rule {
    id: usize,
    /// The speaker the rule belongs to (Who criterion, NPC files = orator,
    /// or the character file); None for shared rules any speaker can use.
    speaker: Option<String>,
    criteria: Vec<Criterion>,
    response: String,
    /// ApplyContext "Name:Value:Duration" entries.
    contexts: Vec<(String, String, f32)>,
    to_world: bool,
    once: bool,
}
/// Source's criterion matcher: "value", "!=value", ">n", ">=n", "<n", "<=n",
/// or two comparisons "a,b" that must both hold. A fact missing from the
/// query compares as "" (0).
fn matches(fact: &str, value: &str) -> bool {
    value.split(',').map(str::trim).filter(|v| !v.is_empty()).all(|v| {
        let num = |x: &str| x.trim().parse::<f32>().ok();
        let fact_num = || num(fact).unwrap_or(0.);
        for (op, f) in [
            (">=", (|a: f32, b: f32| a >= b) as fn(f32, f32) -> bool),
            ("<=", |a, b| a <= b),
            ("!=", |a, b| a != b),
            (">", |a, b| a > b),
            ("<", |a, b| a < b),
        ] {
            if let Some(rest) = v.strip_prefix(op) {
                return match num(rest) {
                    Some(n) if op != "!=" || num(fact).is_some() || fact.is_empty() => f(fact_num(), n),
                    _ => op == "!=" && !fact.eq_ignore_ascii_case(rest),
                };
            }
        }
        match num(v) {
            Some(n) if num(fact).is_some() || fact.is_empty() => fact_num() == n,
            _ => fact.eq_ignore_ascii_case(v),
        }
    })
}
#[derive(Clone, Default)]
struct Scene {
    events: Vec<(f32, String, String)>,
    duration: f32,
}
pub struct Dialogue {
    worker: SceneWorker,
    requested: std::collections::HashSet<String>,
    deferred: Vec<DeferredScene>,
    groups: HashMap<(String, String), Vec<Response>>,
    concepts: HashMap<(String, String), String>,
    scenes: HashMap<String, Scene>,
    pending: Vec<(f32, Action)>,
    choice: usize,
    roster: Vec<String>,
    warned: std::collections::HashSet<String>,
    observed: HashMap<String, (f32, bool)>,
    bark_until: HashMap<String, f32>,
    whitaker_alarm: bool,
    /// Survivor talker names by map-I/O actor id (0 = the player, n = bot n-1).
    actors: Vec<String>,
    /// Rules by concept.
    rules: HashMap<String, Vec<Rule>>,
    /// Response contexts: "world" or a speaker, then the key -> (value, expiry).
    contexts: HashMap<(String, String), (String, f32)>,
    spent: std::collections::HashSet<usize>,
    map: String,
    seed: u32,
    /// When each speaker's current speech ends (the "speaking" fact).
    talk_until: HashMap<String, f32>,
}
struct SceneWorker {
    requests: std::sync::mpsc::Sender<String>,
    results: std::sync::Mutex<std::sync::mpsc::Receiver<(String, Scene, bool)>>,
}
struct DeferredScene {
    path: String,
    speaker: Option<String>,
    ent: Option<usize>,
    start: f32,
    follow: Option<(Option<String>, String, f32)>,
    fire: Option<(String, String, f32)>,
}
/// Seconds of audio in a PCM WAV (0 when unknown).
pub(crate) fn wav_seconds(b: &[u8]) -> f32 {
    if b.get(0..4) != Some(b"RIFF") {
        return 0.;
    }
    let mut at = 12;
    let mut rate = 0u32;
    while at + 8 <= b.len() {
        let size = u32::from_le_bytes(b[at + 4..at + 8].try_into().unwrap()) as usize;
        if &b[at..at + 4] == b"fmt " && at + 20 <= b.len() {
            rate = u32::from_le_bytes(b[at + 16..at + 20].try_into().unwrap());
        }
        if &b[at..at + 4] == b"data" && rate > 0 {
            return size as f32 / rate as f32;
        }
        at += 8 + size + size % 2;
    }
    0.
}
impl SceneWorker {
    fn new(mut assets: SourceAssets) -> Self {
        let (requests, work) = std::sync::mpsc::channel::<String>();
        let (done, results) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let registry = crate::source_sound::Registry::load(&mut assets).ok();
            let mut lengths = HashMap::<String, f32>::new();
            while let Ok(path) = work.recv() {
                let (mut parsed, missing) = match assets.read(&path) {
                    Ok(b) if !b.starts_with(b"bvcd") => {
                        (scene(&String::from_utf8_lossy(&b)), false)
                    }
                    _ => (Scene::default(), true),
                };
                // A speak event lasts as long as its sound (the scene file
                // cache stores the real durations; VCD end times can be short).
                for (at, kind, sound) in &parsed.events {
                    if kind != "speak" {
                        continue;
                    }
                    let key = sound.to_lowercase();
                    let length = *lengths.entry(key.clone()).or_insert_with(|| {
                        registry
                            .as_ref()
                            .and_then(|r| r.definitions.get(&key))
                            .and_then(|d| d.waves.first())
                            .and_then(|w| assets.read(w).ok())
                            .map_or(0., |b| wav_seconds(&b))
                    });
                    parsed.duration = parsed.duration.max(at + length);
                }
                if done.send((path, parsed, missing)).is_err() {
                    break;
                }
            }
        });
        Self {
            requests,
            results: std::sync::Mutex::new(results),
        }
    }
}
// Keep lines for response alternatives: `then` belongs to the preceding scene/print.
fn blocks(text: &str) -> Vec<(String, String, Vec<Vec<String>>)> {
    let mut result = vec![];
    let mut header = None;
    let mut body = vec![];
    let mut depth = 0;
    for line in text.lines() {
        let tokens = kv::tokens(line);
        if tokens.is_empty() {
            continue;
        }
        if depth == 0
            && tokens.len() >= 2
            && matches!(tokens[0].to_lowercase().as_str(), "response" | "rule")
        {
            header = Some((tokens[0].to_lowercase(), tokens[1].to_lowercase()));
        }
        let opened = tokens.iter().filter(|t| t.as_str() == "{").count();
        let closed = tokens.iter().filter(|t| t.as_str() == "}").count();
        if depth > 0 && opened == 0 && closed == 0 {
            body.push(tokens);
        }
        depth += opened;
        depth = depth.saturating_sub(closed);
        if closed > 0 && depth == 0 {
            if let Some((kind, name)) = header.take() {
                result.push((kind, name, std::mem::take(&mut body)));
            } else {
                body.clear();
            }
        }
    }
    result
}
fn scene(text: &str) -> Scene {
    let t = kv::tokens(text);
    let mut result = Scene::default();
    let mut i = 0;
    while i + 3 < t.len() {
        if t[i] != "event" || t[i + 3] != "{" {
            i += 1;
            continue;
        }
        let kind = t[i + 1].clone();
        let start = i + 4;
        let mut end = start;
        let mut depth = 1;
        while end < t.len() && depth > 0 {
            match t[end].as_str() {
                "{" => depth += 1,
                "}" => depth -= 1,
                _ => {}
            }
            end += 1;
        }
        let block = &t[start..end.saturating_sub(1)];
        let time = block.iter().position(|x| x == "time");
        let at = time
            .and_then(|j| block.get(j + 1))
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(0.)
            .max(0.);
        let until = time
            .and_then(|j| block.get(j + 2))
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(at);
        result.duration = result.duration.max(at).max(until);
        if let Some(p) = block.windows(2).find(|p| p[0] == "param") {
            if matches!(kind.as_str(), "speak" | "firetrigger") {
                result.events.push((at, kind, p[1].clone()));
            }
        }
        i = end;
    }
    result
}
impl Dialogue {
    pub fn load(root: &Path) -> io::Result<Self> {
        let mut assets = SourceAssets::new(root, vec![])?;
        let mut groups = HashMap::new();
        let mut concepts = HashMap::new();
        // The talker database is scripts/talker/terror_player.txt and its
        // #include chain (the update copy, which merges the DLC files).
        let mut texts: Vec<(String, String)> = vec![];
        let mut queue = std::collections::VecDeque::from(["talker/terror_player.txt".to_string()]);
        let mut seen = std::collections::HashSet::new();
        while let Some(file) = queue.pop_front() {
            if !seen.insert(file.to_lowercase()) {
                continue;
            }
            let Ok(bytes) = assets.read(&format!("scripts/{file}")) else { continue };
            let text = String::from_utf8_lossy(&bytes).into_owned();
            for line in text.lines() {
                let t = kv::tokens(line);
                if t.len() >= 2 && t[0].eq_ignore_ascii_case("#include") {
                    queue.push_back(t[1].replace('\\', "/"));
                }
            }
            let stem = file.rsplit('/').next().unwrap_or("").trim_end_matches(".txt").to_lowercase();
            texts.push((stem, text));
        }
        // criterion "Name" "Key" "Value" [required] [weight N] from every file.
        let mut criteria = HashMap::<String, (String, String)>::new();
        let mut table = HashMap::<String, Criterion>::new();
        let mut groups_pending: Vec<(String, Vec<String>, bool)> = vec![];
        for (_, text) in &texts {
            for line in text.lines() {
                let t = kv::tokens(line.split("//").next().unwrap_or(""));
                if t.len() >= 4 && t[0].eq_ignore_ascii_case("criterion") && t[2] == "{" {
                    let close = t.iter().position(|x| x == "}").unwrap_or(t.len());
                    groups_pending.push((t[1].to_lowercase(), t[3..close].iter().map(|x| x.to_lowercase()).collect::<Vec<_>>(), t[close..].iter().any(|x| x.eq_ignore_ascii_case("required"))));
                    continue;
                }
                if t.len() >= 4 && t[0].eq_ignore_ascii_case("criterion") {
                    criteria.insert(t[1].to_lowercase(), (t[2].to_lowercase(), t[3].to_lowercase()));
                    let weight = t
                        .iter()
                        .position(|x| x.eq_ignore_ascii_case("weight"))
                        .and_then(|i| t.get(i + 1))
                        .and_then(|w| w.parse().ok())
                        .unwrap_or(1.);
                    table.insert(
                        t[1].to_lowercase(),
                        Criterion {
                            key: t[2].to_lowercase(),
                            value: t[3].clone(),
                            required: t[4..].iter().any(|x| x.eq_ignore_ascii_case("required")),
                            weight,
                            any: vec![],
                            name: t[1].to_lowercase(),
                        },
                    );
                }
            }
        }
        for (name, members, required) in groups_pending {
            let any = members.iter().filter_map(|m| table.get(m)).map(|c| (c.key.clone(), c.value.clone())).collect();
            table.insert(name.clone(), Criterion { key: String::new(), value: String::new(), required, weight: 1., any, name });
        }
        let mut rules = HashMap::<String, Vec<Rule>>::new();
        let mut scores = HashMap::<(String, String), usize>::new();
        let mut rules_seen = 0;
        for (stem, text) in &texts {
            // A character file speaks as that character; shared files
            // (terror_player*, conversation, finales) leave it to the rule.
            let base = stem.split("_dlc").next().unwrap_or(stem);
            let file_who = TALKERS.contains(&base).then_some(base);
            let who = file_who.unwrap_or("");
            for (kind, name, lines) in blocks(text) {
                if kind == "response" {
                    let choices = lines
                        .iter()
                        .filter(|t| t.len() >= 2 && matches!(t[0].as_str(), "scene" | "print"))
                        .map(|t| {
                            let then = t
                                .iter()
                                .position(|x| x.eq_ignore_ascii_case("then"))
                                .and_then(|i| {
                                    Some((
                                        t.get(i + 1)?.to_lowercase(),
                                        t.get(i + 2)?.to_lowercase(),
                                        t.get(i + 3)?.to_lowercase(),
                                        t.get(i + 4)?.parse::<f32>().ok()?,
                                    ))
                                });
                            let delay = t
                                .iter()
                                .position(|x| x.eq_ignore_ascii_case("predelay"))
                                .and_then(|i| t.get(i + 1))
                                .and_then(|v| v.parse().ok())
                                .unwrap_or(0.);
                            Response {
                                scene: (t[0] == "scene").then(|| t[1].to_lowercase()),
                                then,
                                delay,
                                fire: t
                                    .iter()
                                    .position(|x| x.eq_ignore_ascii_case("fire"))
                                    .and_then(|i| {
                                        Some((
                                            t.get(i + 1)?.clone(),
                                            t.get(i + 2)?.clone(),
                                            t.get(i + 3)?.parse().ok()?,
                                        ))
                                    }),
                            }
                        })
                        // A survivor never speaks another survivor's scene. The
                        // retail data has one such slip (producer
                        // C13M3Tanker2Producer -> scenes/coach/lookout03.vcd).
                        .filter(|r| r.scene.as_deref().is_none_or(|path| own_scene(who, path)))
                        .collect::<Vec<_>>();
                    if !choices.is_empty() {
                        groups.entry((who.to_string(), name)).or_insert(choices);
                    }
                } else {
                    let Some(c) = lines.iter().find(|t| t[0].eq_ignore_ascii_case("criteria"))
                    else {
                        continue;
                    };
                    let solo_or_versus = c.iter().any(|t| {
                        t.eq_ignore_ascii_case("IsAlone") || t.eq_ignore_ascii_case("IsVersus")
                    });
                    let Some(concept) = c.iter().skip(1).find_map(|t| {
                        criteria
                            .get(&t.to_lowercase())
                            .filter(|(key, _)| key == "concept")
                            .map(|(_, value)| value.clone())
                            .or_else(|| t.strip_prefix("Concept").map(str::to_lowercase))
                    }) else {
                        continue;
                    };
                    let Some(response) = lines
                        .iter()
                        .find(|t| t.len() > 1 && t[0].eq_ignore_ascii_case("response"))
                    else {
                        continue;
                    };
                    // Speaker: the rule's Who criterion (survivors) or name
                    // criterion (NPC speakers are "orator"), else the file's.
                    let speaker = c
                        .iter()
                        .skip(1)
                        .find_map(|t| match criteria.get(&t.to_lowercase()) {
                            Some((key, value)) if key == "who" => Some(talker_name(value)),
                            Some((key, value)) if key == "name" => Some(value.clone()),
                            _ => None,
                        })
                        .or_else(|| file_who.map(str::to_string))
                        .unwrap_or_else(|| "orator".into());
                    let mut contexts = vec![];
                    for line in &lines {
                        if line.len() > 1 && line[0].eq_ignore_ascii_case("applycontext") {
                            for entry in line[1].split(',') {
                                let mut parts = entry.trim().splitn(3, ':');
                                let (Some(k), Some(v)) = (parts.next(), parts.next()) else { continue };
                                let duration = parts.next().and_then(|d| d.trim().parse().ok()).unwrap_or(0.);
                                contexts.push((k.trim().to_lowercase(), v.trim().to_string(), duration));
                            }
                        }
                    }
                    let flag = |name: &str| lines.iter().any(|l| l.iter().any(|t| t.eq_ignore_ascii_case(name)));
                    let rule_speaker = if ORATORS.contains(&who) {
                        Some("orator".to_string())
                    } else {
                        c.iter()
                            .skip(1)
                            .find_map(|t| match criteria.get(&t.to_lowercase()) {
                                Some((key, value)) if key == "who" => Some(talker_name(value)),
                                _ => None,
                            })
                            .or_else(|| file_who.map(str::to_string))
                    };
                    let list = rules.entry(concept.clone()).or_default();
                    list.push(Rule {
                        id: list.len() + rules_seen,
                        speaker: rule_speaker,
                        criteria: c.iter().skip(1).filter_map(|t| table.get(&t.to_lowercase()).cloned()).collect(),
                        response: response[1].to_lowercase(),
                        contexts,
                        to_world: flag("applycontexttoworld"),
                        once: flag("matchonce"),
                    });
                    rules_seen += 1;
                    let key = (speaker, concept);
                    if !solo_or_versus && scores.get(&key).is_none_or(|n| c.len() < *n) {
                        scores.insert(key.clone(), c.len());
                        concepts.insert(key, response[1].to_lowercase());
                    }
                }
            }
        }
        println!(
            "Native dialogue: {} response groups, {} rules, {} concept bindings",
            groups.len(),
            rules.values().map(Vec::len).sum::<usize>(),
            concepts.len()
        );
        Ok(Self {
            worker: SceneWorker::new(assets),
            requested: Default::default(),
            deferred: vec![],
            groups,
            concepts,
            scenes: HashMap::new(),
            pending: vec![],
            choice: 0,
            roster: vec![
                "mechanic".into(),
                "producer".into(),
                "coach".into(),
                "gambler".into(),
            ],
            warned: Default::default(),
            observed: Default::default(),
            bark_until: Default::default(),
            whitaker_alarm: false,
            actors: vec![],
            rules,
            contexts: HashMap::new(),
            spent: Default::default(),
            map: String::new(),
            seed: 0x2545_f491,
            talk_until: HashMap::new(),
        })
    }
    pub fn set_map(&mut self, map: &str) {
        self.map = map.to_ascii_lowercase();
    }
    pub fn set_actors(&mut self, actors: Vec<String>) {
        self.actors = actors.iter().map(|a| talker_name(a)).collect();
    }
    pub fn reset(&mut self) {
        self.talk_until.clear();
        self.contexts.clear();
        self.spent.clear();
        self.pending.clear();
        self.choice = 0;
        self.observed.clear();
        self.bark_until.clear();
        self.whitaker_alarm = false;
        self.deferred.clear();
    }
    fn schedule_scene(
        &mut self,
        path: &str,
        ent: Option<usize>,
        now: f32,
        follow: Option<(Option<String>, String, f32)>,
    ) {
        self.schedule_scene_actions(path, None, ent, now, follow, None);
    }
    fn schedule_scene_actions(
        &mut self,
        path: &str,
        speaker: Option<String>,
        ent: Option<usize>,
        now: f32,
        follow: Option<(Option<String>, String, f32)>,
        fire: Option<(String, String, f32)>,
    ) {
        if let Some(s) = self.scenes.get(path).cloned() {
            self.enqueue_scene(s, speaker, ent, now, follow, fire);
            return;
        }
        if self.requested.insert(path.into()) {
            let _ = self.worker.requests.send(path.into());
        }
        // Busy while the scene loads; enqueue_scene sets the real end.
        if let Some(who) = &speaker {
            self.talk_until.insert(who.clone(), now + 0.5);
        }
        self.deferred.push(DeferredScene {
            path: path.into(),
            speaker,
            ent,
            start: now,
            follow,
            fire,
        });
    }
    fn poll_scenes(&mut self, now: f32) {
        let ready = self
            .worker
            .results
            .lock()
            .unwrap()
            .try_iter()
            .collect::<Vec<_>>();
        for (path, parsed, missing) in ready {
            if missing && self.warned.insert(path.clone()) {
                eprintln!("Dialogue: missing/compiled scene {path}");
            }
            self.scenes.insert(path, parsed);
        }
        let pending = std::mem::take(&mut self.deferred);
        for request in pending {
            if let Some(scene) = self.scenes.get(&request.path).cloned() {
                self.enqueue_scene(
                    scene,
                    request.speaker,
                    request.ent,
                    request.start.max(now),
                    request.follow,
                    request.fire,
                );
            } else {
                self.deferred.push(request);
            }
        }
    }
    /// Fast-forward integration tests must not turn disk/thread latency into
    /// simulated dialogue delay. Keep production's nonblocking loader intact.
    #[cfg(test)]
    pub(crate) fn wait_for_scene_loads(&mut self, now: f32) {
        let deadline=std::time::Instant::now()+std::time::Duration::from_secs(5);
        while !self.deferred.is_empty() {
            self.poll_scenes(now);
            if self.deferred.is_empty() {break;}
            assert!(std::time::Instant::now()<deadline,"scene worker did not finish integration-test requests");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    fn enqueue_scene(
        &mut self,
        s: Scene,
        speaker: Option<String>,
        ent: Option<usize>,
        now: f32,
        follow: Option<(Option<String>, String, f32)>,
        fire: Option<(String, String, f32)>,
    ) {
        // The "speaking" fact: busy until the scene's speech ends.
        if let Some(who) = speaker {
            let until = self.talk_until.entry(who).or_insert(0.);
            *until = until.max(now + s.duration);
        }
        if let Some((target, input, delay)) = fire {
            self.pending
                .push((now + delay.max(0.), Action::Input(target, input)));
        }
        for (at, kind, param) in s.events {
            let action = if kind == "speak" {
                Some(Action::Speak(param))
            } else {
                ent.map(|id| Action::Output(id, format!("OnTrigger{param}")))
            };
            if let Some(action) = action {
                self.pending.push((now + at, action));
            }
        }
        if let Some(id) = ent {
            self.pending
                .push((now + s.duration, Action::Output(id, "OnCompletion".into())));
        }
        // CAI_ExpresserWithFollowup: a delay >= 0 counts from the end of the
        // speech; a negative delay is queued |delay| after the speech starts
        // (the talker files author "-<line length>" to answer on the last word).
        if let Some((who, concept, delay)) = follow {
            let at = if delay >= 0. { s.duration + delay } else { -delay };
            self.pending.push((now + at, Action::Concept(who, concept)));
        }
    }
    /// Speaks a response-rules concept for one survivor (e.g. the retail
    /// "GooedBySpitter" concept), with the same 8 s bark cooldown.
    pub fn speak(&mut self, who: &str, concept: &str, now: f32) {
        let who = &talker_name(who);
        if self.bark_until.get(who).is_some_and(|at| *at > now) {
            return;
        }
        self.bark_until.insert(who.into(), now + 8.);
        self.concept(Some(who.into()), concept.into(), now);
    }
    pub fn bark(&mut self, who: &str, group: &str, now: f32) {
        let who = &talker_name(who);
        if self.bark_until.get(who).is_some_and(|at| *at > now) {
            return;
        }
        self.bark_until.insert(who.into(), now + 8.);
        self.response(who, group, now);
    }
    pub fn observe(&mut self, who: &str, health: f32, reloading: bool, now: f32) {
        let who = &talker_name(who);
        let previous = self.observed.insert(who.into(), (health, reloading));
        if let Some((before, reload)) = previous {
            let group = if health < before - 0.1 {
                Some("SurvivorMinorHurt")
            } else if health > before + 5. {
                Some("PlayerRelaxedSigh")
            } else if reloading && !reload {
                Some("SurvivorReloading")
            } else {
                None
            };
            if let Some(group) = group {
                self.bark(who, &format!("{group}{who}"), now);
            }
        }
    }
    /// The response alternatives `who` may speak for `group`. Responses from
    /// shared files are keyed ""; an NPC can use another NPC file's response;
    /// a survivor never speaks a scene from another survivor's folder.
    fn choices(&self, who: &str, group: &str) -> Vec<Response> {
        let key = |w: &str| (w.to_string(), group.to_string());
        let found = self.groups.get(&key(who)).or_else(|| {
            self.groups.get(&key("")).or_else(|| {
                (!SURVIVOR_VOICES.contains(&who))
                    .then(|| ORATORS.iter().find_map(|o| self.groups.get(&key(o))))
                    .flatten()
            })
        });
        found
            .map(|c| c.iter().filter(|r| r.scene.as_deref().is_none_or(|path| own_scene(who, path))).cloned().collect())
            .unwrap_or_default()
    }
    pub fn response(&mut self, who: &str, group: &str, now: f32) {
        let who = &talker_name(who);
        let group = group.to_lowercase();
        let choices = self.choices(who, &group);
        if choices.is_empty() {
            return;
        }
        let response = choices[self.choice % choices.len()].clone();
        self.choice += 1;
        let follow = response.then.map(|(target, concept, context, delay)| {
            let target = if target == "self" {
                Some(who.into())
            } else if target == "any" {
                None
            } else {
                Some(target)
            };
            // The follow-up query carries the context pairs ("foo:0" is the
            // authors' placeholder) after a '|'.
            // "From" is the issuer of the follow-up.
            (target, format!("{concept}|{context},from:{who}"), delay)
        });
        if let Some(path) = response.scene {
            self.schedule_scene_actions(&path, Some(who.clone()), None, now + response.delay, follow, response.fire);
        } else if let Some((target, concept, delay)) = follow {
            self.pending.push((
                now + response.delay + delay.max(0.),
                Action::Concept(target, concept),
            ));
        }
    }
    fn concept(&mut self, who: Option<String>, concept: String, now: f32) {
        // "concept|Key:Value,Key:Value" carries a follow-up's context.
        let (concept, context) = concept.split_once('|').unwrap_or((concept.as_str(), ""));
        let extra: Vec<(String, String)> = context
            .split(',')
            .filter_map(|pair| {
                let (k, v) = pair.trim().split_once(':')?;
                Some((k.trim().to_lowercase(), v.trim().to_string()))
            })
            .collect();
        let concept = concept
            .split(|c: char| c == ',' || c.is_whitespace())
            .next()
            .unwrap_or("")
            .to_lowercase();
        // Native Whitaker nags require IsNotSaidC1M2AlarmSetoff. Stop queued
        // requests once the store alarm/cola handoff has advanced the event.
        if matches!(concept.as_str(), "c1m2alarmdoor" | "c1m2colaindoor") {
            self.whitaker_alarm = true;
        }
        if self.whitaker_alarm
            && (concept == "whitaker.nags" || concept.starts_with("c1m2firstoutside"))
        {
            return;
        }
        let who = match who {
            // A map trigger's !activator: the survivor that touched it.
            Some(w) if w.starts_with("!actor") => {
                match w["!actor".len()..].parse::<usize>().ok().and_then(|i| self.actors.get(i)) {
                    Some(name) => Some(name.clone()),
                    None => return,
                }
            }
            Some(w) if w.starts_with("orator:") => Some(w.to_lowercase()),
            other => other.map(|w| talker_name(w.trim_start_matches('!'))),
        };
        let candidates: Vec<String> = match who {
            Some(w) if w == "orator" || w.starts_with("orator:") => vec![w],
            Some(w) => vec![w],
            None => self.roster.clone(),
        };
        let mut best: Option<(f32, String, Rule)> = None;
        let mut ties = 0u32;
        for speaker in &candidates {
            let (speaker, name, actor) = match speaker.strip_prefix("orator:") {
                Some(rest) => {
                    let (name, actor) = rest.split_once('#').unwrap_or((rest, ""));
                    ("orator".to_string(), name.to_string(), actor.parse::<usize>().ok())
                }
                None if speaker == "orator" => ("orator".to_string(), "orator".to_string(), None),
                None => (speaker.clone(), String::new(), None),
            };
            let mut extra = extra.clone();
            // WhoDidIt: the survivor whose action fired the map I/O (the
            // player when the activator is unknown).
            if let Some(who) = self.actors.get(actor.unwrap_or(0)).or(self.roster.first()) {
                extra.push(("whodidit".into(), who.clone()));
            }
            let set = self.criteria_set(&speaker, &name, &concept, &extra, now);
            let roll = self.rand();
            for rule in self.rules.get(&concept).into_iter().flatten() {
                if self.spent.contains(&rule.id) || rule.speaker.as_ref().is_some_and(|s| *s != speaker) {
                    continue;
                }
                let mut score = 0.;
                let mut ok = true;
                for c in &rule.criteria {
                    if c.test(&set) {
                        score += c.weight;
                    } else if c.required {
                        ok = false;
                        break;
                    }
                }
                if !ok {
                    continue;
                }
                match &best {
                    Some((s, ..)) if score < *s => {}
                    Some((s, ..)) if score == *s => {
                        // Equal speakers: pick one at random (rules keep
                        // their first-match order within one speaker).
                        ties += 1;
                        if best.as_ref().is_some_and(|b| b.1 != speaker) && roll % (ties + 1) == 0 {
                            best = Some((score, speaker.clone(), rule.clone()));
                        }
                    }
                    _ => {
                        ties = 0;
                        best = Some((score, speaker.clone(), rule.clone()));
                    }
                }
            }
        }
        let Some((_, speaker, rule)) = best else {
            // Map I/O drives scripted sequences through orators; facts the
            // rewrite cannot reproduce (e.g. c1m2's ColaDelivered) must not
            // stall them, so fall back to the orator rule with the fewest
            // unmet required criteria.
            if let Some(orator) = candidates.iter().find(|c| c.starts_with("orator")) {
                let mut extra = extra.clone();
                if let Some(who) = self.actors.first().or(self.roster.first()) {
                    extra.push(("whodidit".into(), who.clone()));
                }
                let name = orator.strip_prefix("orator:").and_then(|n| n.split('#').next()).unwrap_or("orator").to_string();
                let set = self.criteria_set("orator", &name, &concept, &extra, now);
                let fallback = self
                    .rules
                    .get(&concept)
                    .into_iter()
                    .flatten()
                    .filter(|r| !self.spent.contains(&r.id) && r.speaker.as_ref().is_none_or(|s| s == "orator"))
                    .min_by_key(|r| r.criteria.iter().filter(|c| c.required && !c.test(&set)).count())
                    .map(|r| r.response.clone());
                if let Some(group) = fallback {
                    if audit_enabled() {
                        println!("Native dialogue concept {concept}: orator/{group} (fallback)");
                    }
                    self.response("orator", &group, now);
                    return;
                }
            }
            if audit_enabled() {
                println!("Native dialogue concept {concept}: no rule matched for {candidates:?}");
                let speaker = candidates.first().cloned().unwrap_or_default();
                let speaker = if speaker.starts_with("orator") { "orator".to_string() } else { speaker };
                let mut extra = extra.clone();
                if let Some(who) = self.actors.first().or(self.roster.first()) {
                    extra.push(("whodidit".into(), who.clone()));
                }
                let set = self.criteria_set(&speaker, "orator", &concept, &extra, now);
                for rule in self.rules.get(&concept).into_iter().flatten() {
                    let failed: Vec<_> = rule.criteria.iter().filter(|c| c.required && !c.test(&set)).map(|c| c.name.clone()).collect();
                    println!("  rule {} speaker {:?}: failed {failed:?}", rule.response, rule.speaker);
                }
            }
            return;
        };
        if audit_enabled() {
            println!("Native dialogue concept {concept}: {speaker}/{}", rule.response);
        }
        let owner = if rule.to_world { "world".to_string() } else { speaker.clone() };
        for (key, value, duration) in &rule.contexts {
            let slot = (owner.clone(), key.clone());
            let value = match value.strip_prefix("++") {
                Some(n) => {
                    let current = self
                        .contexts
                        .get(&slot)
                        .filter(|(_, until)| *until > now)
                        .and_then(|(v, _)| v.parse::<f32>().ok())
                        .unwrap_or(0.);
                    format!("{}", current + n.parse::<f32>().unwrap_or(1.))
                }
                None => value.clone(),
            };
            let until = if *duration > 0. { now + duration } else { f32::INFINITY };
            self.contexts.insert(slot, (value, until));
        }
        if rule.once {
            self.spent.insert(rule.id);
        }
        self.response(&speaker, &rule.response, now);
    }
    fn rand(&mut self) -> u32 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        self.seed
    }
    /// The query's criteria set: concept, speaker, map, mode, a random roll,
    /// the speaker's and the world's live contexts ("world" + key) and any
    /// follow-up context. Facts the rewrite does not track stay absent.
    fn criteria_set(&mut self, speaker: &str, name: &str, concept: &str, extra: &[(String, String)], now: f32) -> HashMap<String, String> {
        let mut set = HashMap::new();
        set.insert("concept".into(), concept.to_string());
        if speaker != "orator" {
            set.insert("who".into(), speaker.to_string());
        }
        if !name.is_empty() {
            set.insert("name".into(), name.to_string());
        }
        set.insert("map".into(), self.map.clone());
        set.insert("gamemode".into(), "coop".into());
        set.insert("randomnum".into(), (self.rand() % 101).to_string());
        set.insert("numberofteamalive".into(), self.roster.len().to_string());
        let speaking = self.talk_until.get(speaker).is_some_and(|t| *t > now);
        set.insert("speaking".into(), if speaking { "1" } else { "0" }.into());
        for ((owner, key), (value, until)) in &self.contexts {
            if *until <= now {
                continue;
            }
            if owner == "world" {
                set.insert(format!("world{key}"), value.clone());
            } else if owner == speaker {
                set.insert(key.clone(), value.clone());
            }
        }
        for (key, value) in extra {
            set.insert(key.clone(), value.clone());
        }
        set
    }
    pub fn tick(&mut self, world: &mut EntityWorld, queue: &mut Queue, roster: &[&str]) {
        let _timing = crate::performance::scope("source_dialogue::tick");
        self.roster = roster.iter().map(|s| talker_name(s)).collect();
        let now = world.time;
        self.poll_scenes(now);
        for id in std::mem::take(&mut world.cine.scene_starts) {
            if let Some(path) = world.ents[id].me.kv("SceneFile") {
                self.schedule_scene(&path.to_lowercase(), Some(id), now, None);
            }
        }
        for (who, concept) in std::mem::take(&mut world.cine.concepts) {
            self.concept(who, concept, now);
        }
        // Bound chains so malformed native/mod scripts cannot spin in one frame.
        for _ in 0..64 {
            let Some(index) = self.pending.iter().position(|(at, _)| *at <= now) else {
                break;
            };
            let (_, action) = self.pending.remove(index);
            match action {
                Action::Speak(name) => {
                    if audit_enabled() {
                        println!("Native speech: {name}");
                    }
                    queue.play(name, None);
                }
                Action::Output(id, output) => world.fire_output_pub(id, &output),
                Action::Concept(who, concept) => self.concept(who, concept, now),
                Action::Input(target, input) => world.queue_input(&target, &input, "", 0., None),
            }
        }
    }
}
fn audit_enabled() -> bool {
    static AUDIT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AUDIT.get_or_init(|| std::env::args().any(|a| a == "--audit-dialogue") || std::env::var_os("L4D2R_AUDIT_DIALOGUE").is_some())
}
const SURVIVOR_VOICES: [&str; 8] =
    ["coach", "gambler", "mechanic", "producer", "namvet", "teengirl", "biker", "manager"];
/// Character talker files (a `_dlcN` suffix belongs to the same character).
const TALKERS: [&str; 15] = [
    "coach", "gambler", "mechanic", "producer", "namvet", "teengirl", "biker", "manager", "whitaker",
    "orator", "virgil", "soldier1", "soldier2", "arenaheli", "pilot",
];
/// Map-placed speakers (func_orator): their lines live in these files and
/// do not name a speaker.
const ORATORS: [&str; 7] = ["orator", "whitaker", "virgil", "soldier1", "soldier2", "arenaheli", "pilot"];
/// The talker/voice name for a survivor character or model name
/// (Zoey's model is survivor_teenangst; her talker and voice are teengirl).
pub fn talker_name(character: &str) -> String {
    let name = character
        .trim_start_matches("models/survivors/survivor_")
        .trim_end_matches(".mdl")
        .to_ascii_lowercase();
    match name.as_str() {
        "teenangst" | "zoey" => "teengirl".into(),
        "francis" => "biker".into(),
        "bill" => "namvet".into(),
        "louis" => "manager".into(),
        "nick" => "gambler".into(),
        "ellis" => "mechanic".into(),
        "rochelle" => "producer".into(),
        _ => name,
    }
}
/// Whether a response scene belongs to `who`: survivors only speak scenes
/// from their own scenes/<name>/ folder.
fn own_scene(who: &str, path: &str) -> bool {
    let folder = path.trim_start_matches("scenes/").split('/').next().unwrap_or("");
    !SURVIVOR_VOICES.contains(&who) || !SURVIVOR_VOICES.contains(&folder) || folder == who
}
/// (actor, sound) for every speak event in a VCD; the actor is the
/// enclosing `actor "Name" { ... }` block.
fn scene_speakers(text: &str) -> Vec<(String, String)> {
    let t = kv::tokens(text);
    let mut out = vec![];
    let mut actors: Vec<(String, usize)> = vec![];
    let mut depth = 0usize;
    let mut i = 0;
    while i < t.len() {
        match t[i].as_str() {
            "{" => depth += 1,
            "}" => {
                depth = depth.saturating_sub(1);
                while actors.last().is_some_and(|(_, d)| *d > depth) {
                    actors.pop();
                }
            }
            "actor" if t.get(i + 2).is_some_and(|x| x == "{") => {
                actors.push((t[i + 1].to_lowercase(), depth + 1));
            }
            "event" if t.get(i + 1).is_some_and(|x| x == "speak") => {
                if let Some(j) = (i..t.len().min(i + 40)).find(|&j| t[j] == "param") {
                    let actor = actors.last().map_or(String::new(), |(a, _)| a.clone());
                    out.push((actor, t[j + 1].to_lowercase()));
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn criterion_matcher_follows_source_syntax() {
        assert!(matches("Coach", "coach"));
        assert!(!matches("Gambler", "Coach"));
        assert!(matches("", "!=1"), "a missing fact is empty");
        assert!(!matches("1", "!=1"));
        assert!(matches("", "<1") && matches("", "0"));
        assert!(matches("3", ">1") && !matches("1", ">1"));
        assert!(matches("50", ">=0,<=100") && !matches("101", ">=0,<=100"));
        assert!(matches("versus", "versus") && !matches("coop", "!=coop"));
        assert!(matches("coop", "!=versus"));
    }
    /// Map speakers (func_orator), !activator triggers and the shared/DLC
    /// talker files resolve through terror_player.txt's include chain.
    #[test]
    fn map_speakers_activators_and_dlc_files_resolve() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut d = Dialogue::load(root).unwrap();
        for concept in ["c1m1_elevator_start", "c3m4button101", "c5m5_soldier1", "c2m5_pilotnag"] {
            assert!(
                ORATORS.iter().any(|o| d.concepts.contains_key(&(o.to_string(), concept.to_string()))),
                "{concept}"
            );
        }
        // finales_dlc2.txt (L4D1 finales) binds through its IsNamVet criteria.
        assert!(d.concepts.contains_key(&("namvet".to_string(), "concepthospitalradiotalk".to_string())));
        // !activator = the touching actor: actor 1 is the first bot.
        d.set_actors(vec!["coach".into(), "gambler".into()]);
        d.concept(Some("!actor1".into()), "c1m1_enter_elevator".into(), 0.);
        assert!(d.requested.iter().any(|p| p.starts_with("scenes/gambler/")), "{:?}", d.requested);
        assert!(!d.requested.iter().any(|p| p.starts_with("scenes/coach/")));
    }
    /// Every response a survivor can give speaks only that survivor's voice
    /// (their sound/player/survivor/voice/<name>/ waves).
    #[test]
    fn survivor_responses_only_use_their_own_voice() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let dialogue = Dialogue::load(root).unwrap();
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        let registry = crate::source_sound::Registry::load(&mut assets).unwrap();
        let mut scenes: HashMap<String, Vec<(String, String)>> = HashMap::new();
        let mut wrong = std::collections::BTreeSet::new();
        let mut checked = 0;
        // Every survivor-keyed group, plus whatever each survivor concept
        // binding resolves to (shared-file responses included).
        let mut spoken: Vec<(String, String, Vec<Response>)> = dialogue
            .groups
            .iter()
            .map(|((who, group), choices)| (who.clone(), group.clone(), choices.clone()))
            .collect();
        for ((who, _), group) in &dialogue.concepts {
            spoken.push((who.clone(), group.clone(), dialogue.choices(who, group)));
        }
        for (who, group, choices) in &spoken {
            if !SURVIVOR_VOICES.contains(&who.as_str()) {
                continue;
            }
            for path in choices.iter().filter_map(|r| r.scene.as_ref()) {
                let speakers = scenes.entry(path.clone()).or_insert_with(|| {
                    assets.read(path).map(|b| scene_speakers(&String::from_utf8_lossy(&b))).unwrap_or_default()
                });
                for (actor, sound) in speakers.iter() {
                    let Some(def) = registry.definitions.get(sound) else { continue };
                    for wave in &def.waves {
                        checked += 1;
                        let wave = wave.to_lowercase().replace('\\', "/");
                        // A line written for a named survivor actor (a multi-
                        // character conversation) is that actor's; any other
                        // line is the speaker's own.
                        let owner = if SURVIVOR_VOICES.contains(&actor.as_str()) { actor.as_str() } else { who.as_str() };
                        if !wave.contains(&format!("/voice/{owner}/")) {
                            wrong.insert(format!("{who}/{group}: {path} actor {actor} -> {sound} = {wave}"));
                        }
                    }
                }
            }
        }
        println!("voice audit: {checked} waves checked, {} wrong", wrong.len());
        for w in wrong.iter().take(40) {
            println!("WRONG {w}");
        }
        assert!(checked > 1000);
        assert!(wrong.is_empty(), "{} responses use another survivor's voice", wrong.len());
    }
    fn deferred_fixture() -> (
        Dialogue,
        std::sync::mpsc::Sender<(String, Scene, bool)>,
        std::sync::mpsc::Receiver<String>,
    ) {
        let (requests, work) = std::sync::mpsc::channel();
        let (done, results) = std::sync::mpsc::channel();
        (
            Dialogue {
                worker: SceneWorker {
                    requests,
                    results: std::sync::Mutex::new(results),
                },
                requested: Default::default(),
                deferred: vec![],
                groups: HashMap::new(),
                concepts: HashMap::new(),
                scenes: HashMap::new(),
                pending: vec![],
                choice: 0,
                roster: vec![],
                warned: Default::default(),
                observed: HashMap::new(),
                bark_until: HashMap::new(),
                whitaker_alarm: false,
                actors: vec![],
                rules: HashMap::new(),
                contexts: HashMap::new(),
                spent: Default::default(),
                map: String::new(),
                seed: 1,
                talk_until: HashMap::new(),
            },
            done,
            work,
        )
    }
    #[test]
    fn delayed_scene_load_keeps_authored_offsets_and_waits_before_followup() {
        let (mut dialogue, done, work) = deferred_fixture();
        dialogue.schedule_scene("scene", Some(9), 1., Some((None, "next".into(), 0.5)));
        dialogue.schedule_scene("scene", None, 1., None);
        assert!(dialogue.pending.is_empty());
        assert_eq!(work.try_iter().collect::<Vec<_>>(), ["scene"]);
        done.send((
            "scene".into(),
            Scene {
                events: vec![(0.25, "speak".into(), "line".into())],
                duration: 2.,
            },
            false,
        ))
        .unwrap();
        dialogue.poll_scenes(10.);
        assert!(dialogue.deferred.is_empty());
        assert!(
            dialogue
                .pending
                .iter()
                .any(|(at, a)| *at == 10.25 && matches!(a, Action::Speak(_)))
        );
        assert!(
            dialogue
                .pending
                .iter()
                .any(|(at, a)| *at == 12. && matches!(a, Action::Output(9, _)))
        );
        assert!(
            dialogue
                .pending
                .iter()
                .any(|(at, a)| *at == 12.5 && matches!(a, Action::Concept(_, _)))
        );
    }
    #[test]
    fn response_fire_waits_for_scene_load_and_uses_authored_delay() {
        let (mut dialogue, done, _) = deferred_fixture();
        dialogue.schedule_scene_actions(
            "scene",
            None,
            None,
            1.,
            None,
            Some(("tanker_destroy_relay".into(), "Trigger".into(), 3.5)),
        );
        assert!(dialogue.pending.is_empty());
        done.send(("scene".into(), Scene::default(), false))
            .unwrap();
        dialogue.poll_scenes(10.);
        assert!(dialogue.pending.iter().any(|(at,a)| *at==13.5 &&
            matches!(a,Action::Input(target,input) if target=="tanker_destroy_relay" && input=="Trigger")));
    }
    #[test]
    fn reset_cancels_old_scene_actions_but_keeps_loaded_cache() {
        let (mut dialogue, done, _) = deferred_fixture();
        dialogue.schedule_scene("scene", None, 1., None);
        dialogue.reset();
        done.send((
            "scene".into(),
            Scene {
                events: vec![(0., "speak".into(), "line".into())],
                duration: 1.,
            },
            false,
        ))
        .unwrap();
        dialogue.poll_scenes(2.);
        assert!(dialogue.pending.is_empty());
        dialogue.schedule_scene("scene", None, 3., None);
        assert!(dialogue.pending.iter().any(|(at, _)| *at == 3.));
    }
    #[test]
    fn nested_vcd_events_keep_native_times() {
        let s = scene(
            "actor Coach { channel audio { event speak Speak { time 0.11 6.45 param Coach_WorldC1M1B01 } } } event firetrigger Trigger1 { time 2 -1 param 1 }",
        );
        assert_eq!(s.events.len(), 2);
        assert_eq!(s.events[1], (2., "firetrigger".into(), "1".into()));
        assert_eq!(s.duration, 6.45);
    }
    #[test]
    fn response_lines_preserve_followup_and_comments() {
        let b = blocks(
            "Response Intro\n{\nscene \"a.vcd\" then mechanic next foo:0 -2.5 // dialogue\nprint \" \" then any end foo:0 4\n}\nRule Intro\n{\ncriteria Conceptintro IsCoach\nResponse Intro\n}",
        );
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].2[0][6], "-2.5");
        assert_eq!(b[1].2[0][1], "Conceptintro");
    }
    #[test]
    #[ignore = "requires the local retail L4D2 installation"]
    fn installed_hotel_intro_plays_native_conversation_chain() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        let mut logic = crate::l4d2r_cinematic::MapLogic::load(
            root,
            &root.join("left4dead2/maps/c1m1_hotel.bsp"),
            false,
        )
        .unwrap();
        let mut dialogue = logic.dialogue.take().unwrap();
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        let sounds = crate::source_sound::Registry::load(&mut assets).unwrap();
        let mut queue = Queue::default();
        let mut speech = vec![];
        for _ in 0..2400 {
            logic.world.tick(1. / 60.);
            dialogue.tick(
                &mut logic.world,
                &mut queue,
                &["mechanic", "producer", "coach", "gambler"],
            );
            for request in queue.0.drain(..) {
                let definition = sounds
                    .definitions
                    .get(&request.name.to_lowercase())
                    .unwrap_or_else(|| panic!("Missing native speech definition {}", request.name));
                assert!(
                    assets.exists(&definition.waves[0]),
                    "Missing WAV for {}",
                    request.name
                );
                speech.push(request.name);
            }
        }
        println!("Verified native Hotel conversation: {speech:?}");
        assert!(
            speech.len() >= 3,
            "Intro must continue beyond the first voice line"
        );
    }
}
#[cfg(test)]
mod elevator_tests {
    use super::*;
    /// Seconds of audio in a PCM WAV.
    fn wav_seconds(b: &[u8]) -> f32 {
        let mut at = 12;
        let mut rate = 0u32;
        while at + 8 <= b.len() {
            let size = u32::from_le_bytes(b[at + 4..at + 8].try_into().unwrap()) as usize;
            if &b[at..at + 4] == b"fmt " {
                rate = u32::from_le_bytes(b[at + 16..at + 20].try_into().unwrap());
            }
            if &b[at..at + 4] == b"data" && rate > 0 {
                return size as f32 / rate as f32;
            }
            at += 8 + size + size % 2;
        }
        0.
    }
    /// The c1m1 elevator: four survivors step in a second apart, the button
    /// starts the car. Prints the speech timeline and returns overlaps.
    pub(super) fn elevator_timeline() -> Vec<(f32, String, f32)> {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        let mut logic = crate::l4d2r_cinematic::MapLogic::load(root, &root.join("left4dead2/maps/c1m1_hotel.bsp"), false).unwrap();
        let mut dialogue = logic.dialogue.take().unwrap();
        dialogue.set_actors(vec!["mechanic".into(), "producer".into(), "coach".into(), "gambler".into()]);
        dialogue.set_map("c1m1_hotel");
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        let sounds = crate::source_sound::Registry::load(&mut assets).unwrap();
        let trigger = (0..logic.world.ents.len())
            .find(|&i| logic.world.ents[i].conns.iter().any(|c| c.param.eq_ignore_ascii_case("c1m1_enter_elevator")))
            .expect("elevator trigger");
        let button = logic.world.find("relay_top_button")[0];
        let mut queue = Queue::default();
        let mut timeline = vec![];
        // Let the intro conversation finish first.
        for frame in 0..(150 * 60) {
            let t = frame as f32 / 60.;
            let at = t - 60.;
            for actor in 0..4u64 {
                if (at - actor as f32).abs() < 1e-3 {
                    logic.world.touch_actor(trigger, actor, None);
                }
            }
            if (at - 5.).abs() < 1e-3 {
                logic.world.input(button, "Trigger", "", None, None);
            }
            logic.world.tick(1. / 60.);
            dialogue.tick(&mut logic.world, &mut queue, &["mechanic", "producer", "coach", "gambler"]);
            dialogue.wait_for_scene_loads(logic.world.time);
            for request in queue.0.drain(..) {
                if at < 0. {
                    continue;
                }
                let seconds = sounds
                    .definitions
                    .get(&request.name.to_lowercase())
                    .and_then(|d| assets.read(&d.waves[0]).ok())
                    .map_or(0., |b| wav_seconds(&b));
                timeline.push((at, request.name, seconds));
            }
        }
        timeline
    }
    #[test]
    fn c1m1_elevator_speech_does_not_overlap() {
        if !Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2\left4dead2\maps\c1m1_hotel.bsp").is_file() {
            return;
        }
        let timeline = elevator_timeline();
        let voice = |n: &str| n.split('_').next().unwrap_or("").to_string();
        let mut overlaps = vec![];
        for (i, (t, name, len)) in timeline.iter().enumerate() {
            println!("ELEVATOR {t:6.2} {len:5.2} {name}");
            if ["Coach", "Gambler", "Mechanic", "Producer", "Whitaker"].contains(&voice(name).as_str()) {
                for (u, other, olen) in &timeline[..i] {
                    if voice(other) != "npc.blank" && *u + *olen > *t + 0.25 && ["Coach", "Gambler", "Mechanic", "Producer"].contains(&voice(other).as_str()) {
                        overlaps.push(format!("{other}@{u:.2}+{olen:.2} vs {name}@{t:.2}"));
                    }
                }
            }
        }
        println!("OVERLAPS {overlaps:?}");
        assert!(overlaps.is_empty(), "{overlaps:?}");
    }
}
/// Concepts requested by gameplay systems that do not own the dialogue
/// (who, concept); `l4d2r_cinematic` speaks them on its next tick.
static REQUESTS: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());
pub fn request(who: &str, concept: &str) {
    if let Ok(mut r) = REQUESTS.lock() {
        if r.len() < 16 {
            r.push((who.to_string(), concept.to_string()));
        }
    }
}
pub fn take_requests() -> Vec<(String, String)> {
    REQUESTS
        .lock()
        .map(|mut r| std::mem::take(&mut *r))
        .unwrap_or_default()
}
