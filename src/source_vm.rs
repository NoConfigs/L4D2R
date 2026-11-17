//! Real Squirrel execution. The language VM is upstream 3.0.7; Valve's 3.0.4
//! extensions/engine API parity are audited separately, never silently stubbed.
use std::{
    collections::HashMap,
    ffi::{CStr, CString, c_char, c_void},
    io,
};
unsafe extern "C" {
    fn rvm_new() -> *mut c_void;
    fn rvm_free(vm: *mut c_void);
    fn rvm_source(vm: *mut c_void, name: *const c_char, code: *const c_char);
    fn rvm_eval(vm: *mut c_void, scope: i32, name: *const c_char, code: *const c_char) -> i32;
    fn rvm_run(vm: *mut c_void, scope: i32, name: *const c_char) -> i32;
    fn rvm_compile(vm: *mut c_void, name: *const c_char, code: *const c_char) -> i32;
    fn rvm_scope(vm: *mut c_void, id: i32, path: *const c_char) -> i32;
    fn rvm_release_scope(vm: *mut c_void, id: i32);
    fn rvm_call(vm: *mut c_void, id: i32, table: *const c_char, function: *const c_char) -> i32;
    fn rvm_think(vm: *mut c_void, id: i32, function: *const c_char, result: *mut f64) -> i32;
    fn rvm_number(vm: *mut c_void, key: *const c_char, value: f64);
    fn rvm_random(
        vm: *mut c_void,
        function: unsafe extern "C" fn(*mut c_void, i32, f64, f64) -> f64,
        context: *mut c_void,
    );
    fn rvm_string(vm: *mut c_void, key: *const c_char, value: *const c_char);
    fn rvm_error(vm: *mut c_void) -> *const c_char;
    fn rvm_log(vm: *mut c_void) -> *const c_char;
    fn rvm_clear_log(vm: *mut c_void);
    fn rvm_options(vm: *mut c_void) -> i32;
    fn rvm_option_key(vm: *mut c_void, index: i32) -> *const c_char;
    fn rvm_option_value(vm: *mut c_void, index: i32) -> f64;
    fn rvm_option_bool_call(
        vm: *mut c_void,
        name: *const c_char,
        argument: *const c_char,
        result: *mut i32,
    ) -> i32;
    fn rvm_commands(vm: *mut c_void) -> i32;
    fn rvm_command_text(vm: *mut c_void, index: i32, field: i32) -> *const c_char;
    fn rvm_command_delay(vm: *mut c_void, index: i32) -> f32;
    fn rvm_command_context(vm: *mut c_void, index: i32, field: i32) -> i32;
    fn rvm_clear_commands(vm: *mut c_void);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn supported_convar_updates_are_real_and_unknown_names_are_not_stubbed() {
        let vm = Vm::new(&HashMap::new()).unwrap();
        vm.eval(0,"convar",r#"Convars.SetValue("pain_pills_decay_rate",0);Assert(Convars.GetFloat("pain_pills_decay_rate")==0);"#).unwrap();
        let commands = vm.commands();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].target, "!convar");
        assert!(
            vm.eval(
                0,
                "unknown convar",
                r#"Convars.SetValue("unknown_engine_variable",1);"#
            )
            .is_err()
        );
    }
    #[test]
    fn director_item_policy_resolves_delegates_overrides_errors_and_cleans_stack() {
        let vm = Vm::new(&HashMap::new()).unwrap();
        assert_eq!(
            vm.director_policy("AllowFallenSurvivorItem", "weapon_first_aid_kit")
                .unwrap(),
            None
        );
        vm.eval(1,"base policy",r#"DirectorOptions.AllowFallenSurvivorItem <- function(name){return name!="weapon_first_aid_kit";};"#).unwrap();
        vm.eval(2, "map policy", "DirectorOptions <- {CommonLimit=12};")
            .unwrap();
        assert_eq!(
            vm.director_policy("AllowFallenSurvivorItem", "weapon_first_aid_kit")
                .unwrap(),
            Some(false)
        );
        assert_eq!(
            vm.director_policy("AllowFallenSurvivorItem", "weapon_pipe_bomb")
                .unwrap(),
            Some(true)
        );
        vm.eval(
            3,
            "local policy",
            "DirectorOptions <- {function AllowFallenSurvivorItem(name){return false;}};",
        )
        .unwrap();
        assert_eq!(
            vm.director_policy("AllowFallenSurvivorItem", "weapon_pipe_bomb")
                .unwrap(),
            Some(false)
        );
        vm.reset_local().unwrap();
        assert_eq!(
            vm.director_policy("AllowFallenSurvivorItem", "weapon_pipe_bomb")
                .unwrap(),
            Some(true)
        );
        vm.eval(
            2,
            "invalid policy",
            "DirectorOptions.AllowFallenSurvivorItem <- function(name){return 7;};",
        )
        .unwrap();
        assert!(
            vm.director_policy("AllowFallenSurvivorItem", "weapon_pipe_bomb")
                .is_err()
        );
        vm.eval(
            2,
            "repair policy",
            "DirectorOptions.AllowFallenSurvivorItem <- function(name){return true;};",
        )
        .unwrap();
        assert_eq!(
            vm.director_policy("AllowFallenSurvivorItem", "weapon_\"quote")
                .unwrap(),
            Some(true)
        );
    }
    #[test]
    fn session_options_copy_defaults_mutation_then_map_and_preserve_callbacks() {
        let vm=Vm::new(&HashMap::from([("mode".into(),r#"MutationOptions <- {CommonLimit=7, function AllowFallenSurvivorItem(name){return name!="weapon_first_aid_kit";}};"#.into())])).unwrap();
        vm.eval(
            0,
            "defaults",
            "BaseScriptedDOTable <- {CommonLimit=30, MobMaxSize=12};",
        )
        .unwrap();
        vm.eval(2, "map options", "MapOptions <- {CommonLimit=4};")
            .unwrap();
        vm.load_challenge_options("mode").unwrap();
        let options = vm.options().unwrap();
        assert_eq!(options["CommonLimit"], 4.);
        assert_eq!(options["MobMaxSize"], 12.);
        assert_eq!(
            vm.director_policy("AllowFallenSurvivorItem", "weapon_first_aid_kit")
                .unwrap(),
            Some(false)
        );
        assert_eq!(
            vm.director_policy("AllowFallenSurvivorItem", "weapon_molotov")
                .unwrap(),
            Some(true)
        );
    }
    #[test]
    fn real_vm_executes_functions_closures_classes_loops_includes_and_options_delegation() {
        let sources = HashMap::from([(
            "child".into(),
            "values <- [2,3,4]; function Sum() { local n=0; foreach(v in values) n+=v; return n; }"
                .into(),
        )]);
        let vm = Vm::new(&sources).unwrap();
        vm.eval(
            1,
            "base",
            "DirectorOptions.CommonLimit <- 30; DirectorOptions.MobMaxSize <- 20;",
        )
        .unwrap();
        vm.eval(2,"map","IncludeScript(\"child.nut\",this); class Scale { factor=2; function Apply(n) {return n*factor;} }; DirectorOptions <- { CommonLimit=Scale().Apply(Sum()) }; ").unwrap();
        assert_eq!(vm.options().unwrap()["CommonLimit"], 18.);
        assert_eq!(vm.options().unwrap()["MobMaxSize"], 20.);
        vm.eval(3,"local","local delta=3; DirectorOptions <- {CommonLimit=1}; function Update(){DirectorOptions.CommonLimit+=delta;} ").unwrap();
        for _ in 0..4 {
            vm.call(3, "", "Update").unwrap();
        }
        assert_eq!(vm.options().unwrap()["CommonLimit"], 13.);
        vm.eval(4, "challenge", "DirectorOptions <- {CommonLimit=5}")
            .unwrap();
        assert_eq!(vm.options().unwrap()["CommonLimit"], 5.);
        vm.eval(
            0,
            "clear challenge",
            "DirectorScript.MapScript.ChallengeScript.DirectorOptions=null;",
        )
        .unwrap();
        // Replace the challenge table to remove its raw override entirely.
        vm.eval(
            0,
            "clear challenge",
            "DirectorScript.MapScript.ChallengeScript={};",
        )
        .unwrap();
        vm.reset_local().unwrap();
        assert_eq!(vm.options().unwrap()["CommonLimit"], 18.);
    }
    #[test]
    fn callbacks_entfire_rng_and_reset_have_real_effects() {
        let vm = Vm::new(&HashMap::new()).unwrap();
        vm.eval(3,"callbacks",r#"DirectorOptions <- {CommonLimit=1};
            function OnGameEvent_lab(params) {DirectorOptions.CommonLimit=params.count.tointeger();EntFire("door","Open","",0.25);}
            __CollectGameEventCallbacks(this);"#).unwrap();
        vm.event("lab", &[("count", "12")]).unwrap();
        assert_eq!(vm.options().unwrap()["CommonLimit"], 12.);
        let commands = vm.commands();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].target, "door");
        assert_eq!(commands[0].delay, 0.25);
        let mut rng = l4d2r_game_core::random::UniformRandomStream::new(1);
        let expected = rng.random_int(1, 50) as f32;
        vm.eval(3, "random", "DirectorOptions.CommonLimit=RandomInt(1,50);")
            .unwrap();
        assert_eq!(vm.options().unwrap()["CommonLimit"], expected);
        vm.reset_local().unwrap();
        vm.event("lab", &[("count", "99")]).unwrap();
        assert!(!vm.options().unwrap().contains_key("CommonLimit"));
        assert!(vm.commands().is_empty());
    }
    #[test]
    fn errors_cycles_and_infinite_loops_leave_vm_usable_and_preserve_accepted_io() {
        let vm = Vm::new(&HashMap::from([
            ("a".into(), "IncludeScript(\"b\",this);".into()),
            ("b".into(), "IncludeScript(\"a\",this);".into()),
        ]))
        .unwrap();
        assert!(
            vm.run(3, "a")
                .unwrap_err()
                .to_string()
                .contains("include cycle")
        );
        assert!(
            vm.eval(
                3,
                "unbound",
                "EntFire(\"door\",\"Kill\"); UnknownEngineFunction();"
            )
            .is_err()
        );
        let accepted = vm.commands();
        assert_eq!(accepted.len(), 1);
        assert_eq!(accepted[0].input, "Kill");
        let began = std::time::Instant::now();
        assert!(
            vm.eval(3, "loop", "while(true) {}")
                .unwrap_err()
                .to_string()
                .contains("budget")
        );
        assert!(began.elapsed() < std::time::Duration::from_secs(1));
        vm.eval(3, "recovery", "DirectorOptions <- {CommonLimit=9};")
            .unwrap();
        for _ in 0..100 {
            assert_eq!(vm.options().unwrap()["CommonLimit"], 9.);
        }
    }
    #[test]
    fn installed_gauntlet_executes_live_recalculation_instead_of_static_option_scanning() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = crate::source_assets::SourceAssets::new(root, vec![]).unwrap();
        let code = crate::source_ice::decode(
            &assets
                .read("scripts/vscripts/director_gauntlet.nuc")
                .unwrap(),
        );
        let vm = Vm::new(&HashMap::from([(
            "director_gauntlet".into(),
            String::from_utf8(code).unwrap(),
        )]))
        .unwrap();
        vm.run(3, "director_gauntlet").unwrap();
        vm.number("Flow", 10000.);
        vm.number("Speed", 200.);
        vm.call(3, "", "Update").unwrap();
        let options = vm.options().unwrap();
        assert_eq!(options["CommonLimit"], 30.);
        assert_eq!(options["MobSpawnSize"], 27.5);
        vm.number("Flow", 0.);
        vm.number("Speed", 50.);
        vm.call(3, "", "Update").unwrap();
        let options = vm.options().unwrap();
        assert_eq!(options["CommonLimit"], 7.5);
        assert_eq!(options["MobSpawnSize"], 5.);
    }
    #[test]
    fn vm_integer_and_float_values_match_native_32_bit_widths() {
        let vm = Vm::new(&HashMap::new()).unwrap();
        vm.eval(
            3,
            "native widths",
            "DirectorOptions <- { Sign = (1 << 31), FloatRound = (16777216.0 + 1.0) };",
        )
        .unwrap();
        let options = vm.options().unwrap();
        assert_eq!(options["Sign"], -2147483648.);
        assert_eq!(options["FloatRound"], 16777216.);
        vm.eval(
            3,
            "format widths",
            "Assert((-1).tostring()==\"-1\"); Assert(format(\"%d\",-1)==\"-1\");",
        )
        .unwrap();
    }
    #[test]
    fn coroutines_share_host_bindings_and_execution_budget() {
        let vm = Vm::new(&HashMap::new()).unwrap();
        vm.eval(3,"coroutine", "local task=newthread(function(){Director.ResetMobTimer();return 7;}); DirectorOptions <- {CommonLimit=task.call()};").unwrap();
        assert_eq!(vm.options().unwrap()["CommonLimit"], 7.);
        assert_eq!(vm.commands()[0].input, "ResetMobTimer");
        assert!(
            vm.eval(
                3,
                "coroutine loop",
                "local task=newthread(function(){while(true){}}); task.call();"
            )
            .is_err()
        );
        vm.eval(3, "after coroutine", "DirectorOptions.CommonLimit=8;")
            .unwrap();
        assert_eq!(vm.options().unwrap()["CommonLimit"], 8.);
    }
    #[test]
    fn l4d2_io_helpers_preserve_self_activator_caller_and_reject_foreign_branch_aliases() {
        let vm = Vm::new(&HashMap::from([("included".into(), "answer <- 9;".into())])).unwrap();
        vm.eval(0,"entities", "__EntityData[2] <- {alive=true,name=\"door\",classname=\"prop_door_rotating\",model=\"door.mdl\",origin=Vector(),angles=Vector()};").unwrap();
        vm.eval(3,"context", "self <- __EntityHandle(2); IncludeScript(\"included\"); Assert(answer==9); EntFire(\"!self\",\"Open\",0,0.25);").unwrap();
        let command = vm.commands().pop().unwrap();
        assert_eq!(command.param, "");
        assert_eq!(command.caller, Some(2));
        assert_eq!(command.activator, Some(2));
        vm.eval(0,"branch surface","Assert(!(\"EntFireByHandle\" in getroottable())); Assert(\"DoIncludeScript\" in getroottable());").unwrap();
    }
    #[test]
    fn valve_event_callbacks_use_registration_weak_scopes_reverse_order_and_script_events() {
        let vm = Vm::new(&HashMap::new()).unwrap();
        vm.eval(0,"events",r#"order <- "";
            a <- {function OnGameEvent_probe(p){::order += "A";}};
            b <- {function OnGameEvent_probe(p){::order += "B";} function OnScriptEvent_test(p){::order += p.text;}};
            __CollectGameEventCallbacks(a); __CollectGameEventCallbacks(b); __CollectGameEventCallbacks(a);
            local ephemeral={function OnGameEvent_probe(p){::order += "X";}};
            __CollectGameEventCallbacks(ephemeral); ephemeral=null; collectgarbage();"#).unwrap();
        vm.event("probe", &[]).unwrap();
        vm.eval(0,"check", "Assert(order==\"BA\"); FireScriptEvent(\"test\",{text=\"S\"}); Assert(order==\"BAS\"); ClearGameEventCallbacks();").unwrap();
        vm.event("probe", &[]).unwrap();
        vm.eval(0, "clear check", "Assert(order==\"BAS\");")
            .unwrap();
    }
    #[test]
    fn entity_queries_use_live_order_full_wildcards_models_and_source_unit_radius() {
        let vm = Vm::new(&HashMap::new()).unwrap();
        vm.eval(0,"query", r#"
            __EntityData[1] <- {alive=true,name="Door_a_Left",classname="prop_door_rotating",model="door.mdl",origin=Vector(10,0,0),angles=Vector()};
            __EntityData[2] <- {alive=false,name="Door_b_Left",classname="prop_door_rotating",model="door.mdl",origin=Vector(),angles=Vector()};
            __EntityData[3] <- {alive=true,name="Door_c_Left",classname="prop_door_rotating",model="door.mdl",origin=Vector(20,0,0),angles=Vector()};
            Assert(Entities.FindByName(null,"door_*_left").id==1);
            Assert(Entities.Next(Entities.First()).id==3);
            Assert(Entities.FindByModel(null,"door.mdl").id==1);
            Assert(Entities.FindByClassnameNearest("prop_door*",Vector(19,0,0),9).id==3);
            Assert(Entities.FindByClassnameWithin(null,"prop_*",Vector(),10).id==1);
            Assert(Entities.FindByClassnameWithin(Entities.First(),"prop_*",Vector(),10)==null);
        "#).unwrap();
    }
    #[test]
    fn releasing_a_private_entity_scope_allows_weak_callbacks_to_expire() {
        let vm = Vm::new(&HashMap::new()).unwrap();
        vm.eval(0,"scope life",r#"
            __EntityData[1] <- {alive=true,name="owner",classname="info_target",model="",origin=Vector(),angles=Vector()};
            local scope=__EntityHandle(1).GetScriptScope();
            scope.OnGameEvent_probe <- function(p){EntFire("marker","Registered");};
            __CollectGameEventCallbacks(scope);
        "#).unwrap();
        vm.scope(1001, "__EntityScopes.e1").unwrap();
        vm.event("probe", &[]).unwrap();
        assert_eq!(vm.commands().len(), 1);
        vm.release_entity_scope(1).unwrap();
        vm.eval(
            0,
            "remove entity",
            "__EntityData[1].alive=false;collectgarbage();",
        )
        .unwrap();
        vm.event("probe", &[]).unwrap();
        assert!(vm.commands().is_empty());
        assert!(
            vm.eval(0, "invalid scope", "__EntityHandle(1).GetScriptScope();")
                .is_err()
        );
    }
}
fn c(s: &str) -> io::Result<CString> {
    CString::new(s).map_err(|_| io::Error::other("NUL in script argument"))
}
unsafe fn copied(p: *const c_char) -> String {
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}
pub fn audit(root: &std::path::Path) -> io::Result<()> {
    let mut assets = crate::source_assets::SourceAssets::new(root, vec![])?;
    let vm = Vm::new(&HashMap::new())?;
    let mut report = String::from("script\tcompiled\terror\n");
    let mut count = 0;
    let mut failures = 0;
    let mut paths = assets.matching_paths("scripts/vscripts/");
    paths.sort();
    for path in paths
        .into_iter()
        .filter(|p| p.ends_with(".nut") || p.ends_with(".nuc"))
    {
        let bytes = assets.read(&path)?;
        let bytes = if path.ends_with(".nuc") {
            crate::source_ice::decode(&bytes)
        } else {
            bytes
        };
        let source = String::from_utf8(bytes.clone())
            .unwrap_or_else(|_| bytes.iter().map(|v| char::from(*v)).collect());
        match vm.compile(&path, &source) {
            Ok(()) => report.push_str(&format!("{path}\tyes\t\n")),
            Err(error) => {
                failures += 1;
                report.push_str(&format!(
                    "{path}\tno\t{}\n",
                    error.to_string().replace(['\t', '\n', '\r'], " ")
                ));
            }
        }
        count += 1;
    }
    std::fs::create_dir_all("local-research")?;
    std::fs::write("local-research/vscript-compile-audit.tsv", report)?;
    println!(
        "Squirrel compile audit: {count} installed sources, {} compiled, {failures} failed (syntax coverage, not engine API/mission completion)",
        count - failures
    );
    if failures > 0 {
        Err(io::Error::other(
            "Some installed scripts did not compile; see vscript-compile-audit.tsv",
        ))
    } else {
        Ok(())
    }
}
pub fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\x00"),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}
#[derive(Debug, Clone)]
pub struct Command {
    pub target: String,
    pub input: String,
    pub param: String,
    pub delay: f32,
    pub activator: Option<usize>,
    pub caller: Option<usize>,
}
pub struct Vm {
    ptr: *mut c_void,
    _random: Box<l4d2r_game_core::random::UniformRandomStream>,
}
unsafe extern "C" fn random(context: *mut c_void, integer: i32, lo: f64, hi: f64) -> f64 {
    let rng = unsafe { &mut *context.cast::<l4d2r_game_core::random::UniformRandomStream>() };
    if integer != 0 {
        rng.random_int(lo as i32, hi as i32) as f64
    } else {
        rng.random_float(lo as f32, hi as f32) as f64
    }
}
// Each VM has independent heap/state and is only accessed behind its owning mutex.
// It may move between Bevy worker threads; Squirrel has no thread-local VM state.
unsafe impl Send for Vm {}
impl Drop for Vm {
    fn drop(&mut self) {
        unsafe {
            rvm_free(self.ptr);
        }
    }
}
impl Vm {
    pub fn compile(&self, name: &str, code: &str) -> io::Result<()> {
        self.check(unsafe { rvm_compile(self.ptr, c(name)?.as_ptr(), c(code)?.as_ptr()) })
    }
    pub fn new(sources: &HashMap<String, String>) -> io::Result<Self> {
        let mut vm = Self {
            ptr: unsafe { rvm_new() },
            _random: Box::new(l4d2r_game_core::random::UniformRandomStream::new(1)),
        };
        if vm.ptr.is_null() {
            return Err(io::Error::other("Squirrel VM allocation failed"));
        }
        unsafe {
            rvm_random(
                vm.ptr,
                random,
                (&mut *vm._random as *mut l4d2r_game_core::random::UniformRandomStream).cast(),
            );
        }
        for (name, code) in sources {
            unsafe {
                rvm_source(vm.ptr, c(name)?.as_ptr(), c(code)?.as_ptr());
            }
        }
        vm.number("Time", 0.);
        vm.number("Flow", 0.);
        vm.number("Speed", 0.);
        vm.number("CommonCount", 0.);
        vm.number("PendingMobCount", 0.);
        vm.number("Intensity", 0.);
        vm.number("InCombat", 0.);
        vm.number("LeftSafeArea", 0.);
        vm.number("Convar.pain_pills_decay_rate", 0.27);
        vm.number("Convar.fall_speed_safe", 560.);
        vm.number("Convar.fall_speed_fatal", 720.);
        vm.number("Convar.survivor_incap_max_fall_damage", 200.);
        vm.string("GameMode", "coop")?;
        vm.string("Difficulty", "Normal")?;
        vm.string("MapName", "")?;
        vm.eval(0, "bootstrap", include_str!("vscript/bootstrap.nut"))?;
        // Values recovered from owned server.dll constant registration 10289480.
        let finales = [
            "GAUNTLET_1",
            "HORDE_ATTACK_1",
            "HALFTIME_BOSS",
            "GAUNTLET_2",
            "HORDE_ATTACK_2",
            "FINAL_BOSS",
            "HORDE_ESCAPE",
            "CUSTOM_PANIC",
            "CUSTOM_TANK",
            "CUSTOM_SCRIPTED",
            "CUSTOM_DELAY",
            "CUSTOM_CLEAROUT",
            "GAUNTLET_START",
            "GAUNTLET_HORDE",
            "GAUNTLET_HORDE_BONUSTIME",
            "GAUNTLET_BOSS_INCOMING",
            "GAUNTLET_BOSS",
            "GAUNTLET_ESCAPE",
        ];
        let mut constants = String::new();
        for (value, key) in finales.iter().enumerate() {
            constants.push_str(&format!("FINALE_{key} <- {value};\n"));
        }
        // Constant registration 1026ef40.
        for (key, value) in [
            ("NO_PREFERENCE", -1),
            ("ANYWHERE", 0),
            ("BEHIND_SURVIVORS", 1),
            ("NEAR_IT_VICTIM", 2),
            ("SPECIALS_IN_FRONT_OF_SURVIVORS", 3),
            ("SPECIALS_ANYWHERE", 4),
            ("FAR_AWAY_FROM_SURVIVORS", 5),
            ("ABOVE_SURVIVORS", 6),
            ("IN_FRONT_OF_SURVIVORS", 7),
            ("VERSUS_FINALE_DISTANCE", 8),
            ("LARGE_VOLUME", 9),
            ("NEAR_POSITION", 10),
        ] {
            constants.push_str(&format!("SPAWN_{key} <- {value};\n"));
        }
        vm.eval(0, "native constants", &constants)?;
        for (id, path) in [
            (1, "DirectorScript"),
            (2, "DirectorScript.MapScript"),
            (3, "DirectorScript.MapScript.LocalScript"),
            (4, "DirectorScript.MapScript.ChallengeScript"),
        ] {
            vm.scope(id, path)?;
        }
        Ok(vm)
    }
    fn check(&self, result: i32) -> io::Result<()> {
        if result != 0 {
            Ok(())
        } else {
            Err(io::Error::other(unsafe { copied(rvm_error(self.ptr)) }))
        }
    }
    pub fn eval(&self, scope: i32, name: &str, code: &str) -> io::Result<()> {
        self.check(unsafe { rvm_eval(self.ptr, scope, c(name)?.as_ptr(), c(code)?.as_ptr()) })
    }
    pub fn take_output(&self) -> String {
        let output = unsafe { copied(rvm_log(self.ptr)) };
        unsafe {
            rvm_clear_log(self.ptr);
        }
        output
    }
    pub fn run(&self, scope: i32, name: &str) -> io::Result<()> {
        self.check(unsafe { rvm_run(self.ptr, scope, c(name)?.as_ptr()) })
    }
    pub fn scope(&self, id: i32, path: &str) -> io::Result<()> {
        self.check(unsafe { rvm_scope(self.ptr, id, c(path)?.as_ptr()) })
    }
    pub fn release_entity_scope(&self, id: usize) -> io::Result<()> {
        self.eval(
            0,
            "entity scope release",
            &format!("if(__EntityScopes.rawin(\"e{id}\")) __EntityScopes.rawdelete(\"e{id}\");"),
        )?;
        unsafe {
            rvm_release_scope(self.ptr, 1000 + id as i32);
        }
        Ok(())
    }
    pub fn call(&self, id: i32, table: &str, function: &str) -> io::Result<()> {
        self.check(unsafe { rvm_call(self.ptr, id, c(table)?.as_ptr(), c(function)?.as_ptr()) })
    }
    pub fn think(&self, id: i32, function: &str) -> io::Result<f32> {
        let mut interval = 0.1;
        self.check(unsafe { rvm_think(self.ptr, id, c(function)?.as_ptr(), &mut interval) })?;
        Ok(interval as f32)
    }
    pub fn collect_callbacks(&self, id: i32) -> io::Result<()> {
        self.eval(
            id,
            "event registration",
            "__CollectGameEventCallbacks(this);",
        )
    }
    pub fn number(&self, key: &str, value: f64) {
        if let Ok(key) = c(key) {
            unsafe {
                rvm_number(self.ptr, key.as_ptr(), value);
            }
        }
    }
    pub fn string(&self, key: &str, value: &str) -> io::Result<()> {
        unsafe {
            rvm_string(self.ptr, c(key)?.as_ptr(), c(value)?.as_ptr());
        }
        Ok(())
    }
    pub fn reset_local(&self) -> io::Result<()> {
        self.eval(0,"EndScript", "__UnregisterScope(DirectorScript.MapScript.LocalScript); DirectorScript.MapScript.LocalScript = {}; DirectorScript.MapScript.LocalScript.setdelegate(DirectorScript.MapScript);")?;
        self.scope(3, "DirectorScript.MapScript.LocalScript")
    }
    /// The owned scriptedmode MergeSessionOptionTables copies defaults, then
    /// MutationOptions, then MapOptions, overwriting duplicate keys in that order.
    /// This connects that options layer; it does not initialize HUD/spawn/finale APIs.
    pub fn load_challenge_options(&self, name: &str) -> io::Result<()> {
        self.string("GameMode", name)?;
        self.eval(0,"reset challenge","__UnregisterScope(DirectorScript.MapScript.ChallengeScript); DirectorScript.MapScript.ChallengeScript={}; DirectorScript.MapScript.ChallengeScript.setdelegate(DirectorScript.MapScript);")?;
        self.scope(4, "DirectorScript.MapScript.ChallengeScript")?;
        self.run(4, name)?;
        self.eval(0,"session options",r#"
            local mode=DirectorScript.MapScript.ChallengeScript;
            if(!mode.rawin("DirectorOptions")) mode.DirectorOptions <- {};
            if(typeof mode.DirectorOptions!="table") throw "DirectorOptions must be a table";
            local copy=function(source,dest){if(typeof source!="table") throw "session options must be a table"; foreach(k,v in source) dest[k] <- v;};
            if("BaseScriptedDOTable" in getroottable()) copy(::BaseScriptedDOTable,mode.DirectorOptions);
            if(mode.rawin("MutationOptions")) copy(mode.MutationOptions,mode.DirectorOptions);
            if(DirectorScript.MapScript.rawin("MapOptions")) copy(DirectorScript.MapScript.MapOptions,mode.DirectorOptions);
            ::SessionOptions <- mode.DirectorOptions;
        "#)?;
        self.collect_callbacks(4)
    }
    pub fn options(&self) -> io::Result<HashMap<String, f32>> {
        let count = unsafe { rvm_options(self.ptr) };
        if count < 0 {
            return self.check(0).map(|_| HashMap::new());
        }
        Ok((0..count)
            .map(|i| unsafe {
                (
                    copied(rvm_option_key(self.ptr, i)),
                    rvm_option_value(self.ptr, i) as f32,
                )
            })
            .collect())
    }
    pub fn director_policy(&self, name: &str, argument: &str) -> io::Result<Option<bool>> {
        let mut result = 0;
        match unsafe {
            rvm_option_bool_call(
                self.ptr,
                c(name)?.as_ptr(),
                c(argument)?.as_ptr(),
                &mut result,
            )
        } {
            1 => Ok(Some(result != 0)),
            0 => Ok(None),
            _ => self.check(0).map(|_| None),
        }
    }
    pub fn commands(&self) -> Vec<Command> {
        let commands = (0..unsafe { rvm_commands(self.ptr) })
            .map(|i| unsafe {
                Command {
                    target: copied(rvm_command_text(self.ptr, i, 0)),
                    input: copied(rvm_command_text(self.ptr, i, 1)),
                    param: copied(rvm_command_text(self.ptr, i, 2)),
                    delay: rvm_command_delay(self.ptr, i),
                    activator: usize::try_from(rvm_command_context(self.ptr, i, 0)).ok(),
                    caller: usize::try_from(rvm_command_context(self.ptr, i, 1)).ok(),
                }
            })
            .collect();
        unsafe {
            rvm_clear_commands(self.ptr);
        }
        commands
    }
    pub fn event(&self, name: &str, params: &[(&str, &str)]) -> io::Result<()> {
        let fields = params
            .iter()
            .map(|(k, v)| format!("{} : {}", quote(k), quote(v)))
            .collect::<Vec<_>>()
            .join(",");
        self.eval(
            0,
            "game event",
            &format!("__DispatchGameEvent({}, {{{fields}}});", quote(name)),
        )
    }
}
