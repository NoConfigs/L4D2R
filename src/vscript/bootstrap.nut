// Engine bindings with actual host state. Unbound engine calls raise an error.
function printl(s) { print(s + "\n"); }
function Msg(s) { print(s); }
function IsLinux() { return false; }
function IsDedicatedServer() { return false; }
function Time() { return __host("Time"); }
function RandomInt(a,b) { return __host("RandomInt",a,b); }
function RandomFloat(a,b) { return __host("RandomFloat",a,b); }
function DoEntFire(target,action,value,delay,activator,caller) {
    __host("EntFire",target,action,value,delay,activator==null?null:activator.id,caller==null?null:caller.id);
}
function EntFire(target,action,value=null,delay=0.0,activator=null) {
    if(!value) value="";
    local caller=null;
    if("self" in this) {
        caller=self;
        if(!activator) activator=self;
    }
    DoEntFire(target.tostring(),action.tostring(),value.tostring(),delay,activator,caller);
}
function IncludeScript(name,scope=null) {
    if(scope==null) scope=this;
    return ::DoIncludeScript(name,scope);
}
function AddThinkToEnt(entity,name) {
    if(entity==null || !entity.IsValid()) throw "invalid think entity";
    __host("Think",entity.id,name);
}
function Assert(test,message="assertion failed") { if(!test) throw message; }
function developer() { return 0; }
class Vector {
    x=0.0; y=0.0; z=0.0;
    constructor(a=0,b=0,c=0) { x=a.tofloat();y=b.tofloat();z=c.tofloat(); }
    function _add(v) { return Vector(x+v.x,y+v.y,z+v.z); }
    function _sub(v) { return Vector(x-v.x,y-v.y,z-v.z); }
    function _mul(s) { return Vector(x*s,y*s,z*s); }
    function _div(s) { return Vector(x/s,y/s,z/s); }
    function Dot(v) { return x*v.x+y*v.y+z*v.z; }
    function Cross(v) { return Vector(y*v.z-z*v.y,z*v.x-x*v.z,x*v.y-y*v.x); }
    function LengthSqr() { return Dot(this); }
    function Length() { return sqrt(LengthSqr()); }
    function Length2D() { return sqrt(x*x+y*y); }
    function Norm() {
        local n=Length();
        if(n>0) {x/=n;y/=n;z/=n;}
        return n;
    }
    function _tostring() { return format("(%g,%g,%g)",x,y,z); }
    // Key/value form "x y z" (c14m2's finale builds spawn origins with it).
    function ToKVString() { return format("%g %g %g",x,y,z); }
}
Director <- {
    function GetGameModeBase() { return __host("GameMode"); }
    function GetGameMode() { return __host("GameMode"); }
    function GetMapName() { return __host("MapName"); }
    function IsSessionStartMap() { return __host("SessionStartMap")!=0; }
    function GetDifficulty() { return __host("Difficulty"); }
    function IsPlayingOnConsole() { return false; }
    function IsSinglePlayerGame() { return true; }
    function GetFurthestSurvivorFlow() { return __host("Flow"); }
    function GetAveragedSurvivorSpeed() { return __host("Speed"); }
    function GetCommonInfectedCount() { return __host("CommonCount").tointeger(); }
    function GetPendingMobCount() {return __host("PendingMobCount").tointeger();}
    function GetIntensity() { return __host("Intensity"); }
    function IsAnySurvivorInCombat() { return __host("InCombat")!=0; }
    function HasAnySurvivorLeftSafeArea() { return __host("LeftSafeArea")!=0; }
    function IsTankInPlay() { return __host("TankInPlay")!=0; }
    // info_director OnUserDefinedScriptEvent1..4 (c7m3 prompts the
    // generator restart with event 1).
    function UserDefinedEvent1() { DoEntFire("!director_event","OnUserDefinedScriptEvent1","",0,null,null); }
    function UserDefinedEvent2() { DoEntFire("!director_event","OnUserDefinedScriptEvent2","",0,null,null); }
    function UserDefinedEvent3() { DoEntFire("!director_event","OnUserDefinedScriptEvent3","",0,null,null); }
    function UserDefinedEvent4() { DoEntFire("!director_event","OnUserDefinedScriptEvent4","",0,null,null); }
    function L4D1SurvivorGiveItem(...) {}
    function ResetMobTimer() { __host("DirectorCommand","ResetMobTimer"); }
    function ForcePanicEvent() { __host("DirectorCommand","ForcePanicEvent"); }
    function PlayMegaMobWarningSounds() { __host("Sound","MegaMobIncoming"); }
};
// L4D1 helper survivors (c6m3's Zoey/Francis/Louis) are not simulated:
// handing them items is a no-op rather than an error that cuts the finale
// stage hook short.
function L4D1SurvivorGiveItem(...) {}
// Global difficulty helper (used by c14m2_lighthouse_finale): 0 Easy,
// 1 Normal, 2 Advanced, 3 Expert.
function GetDifficulty() {
    local d = Director.GetDifficulty();
    if (typeof d != "string") return d;
    switch (d.tolower()) {
        case "easy": return 0;
        case "hard": case "advanced": return 2;
        case "impossible": case "expert": return 3;
    }
    return 1;
}
NavMesh <- {
    function UnblockRescueVehicleNav() { DoEntFire("!navmesh","UnblockRescueVehicleNav","",0,null,null); }
};
Convars <- {
    function GetFloat(name){return __host("ConvarGet",name);}
    function GetInt(name){return __host("ConvarGet",name).tointeger();}
    function SetValue(name,value){__host("ConvarSet",name,value);}
};
DirectorScript <- {};
DirectorScript.setdelegate(getroottable());
DirectorScript.DirectorOptions <- {};
DirectorScript.MapScript <- {};
DirectorScript.MapScript.setdelegate(DirectorScript);
DirectorScript.MapScript.LocalScript <- {};
DirectorScript.MapScript.LocalScript.setdelegate(DirectorScript.MapScript);
DirectorScript.MapScript.ChallengeScript <- {};
DirectorScript.MapScript.ChallengeScript.setdelegate(DirectorScript.MapScript);
SessionOptions <- {};
SessionState <- {};
__EntityData <- {};
__EntityHandles <- {};
__EntityScopes <- {};
class CBaseEntity {
    id=0;
    constructor(index) {id=index;}
    function entindex() {return id;}
    function GetEntityIndex() {return id;}
    function IsValid() {return __EntityData.rawin(id) && __EntityData[id].alive;}
    function GetName() {return __EntityData[id].name;}
    function GetClassname() {return __EntityData[id].classname;}
    function GetModelName() {return __EntityData[id].model;}
    function GetOrigin() {local v=__EntityData[id].origin;return Vector(v.x,v.y,v.z);}
    function GetAngles() {local v=__EntityData[id].angles;return Vector(v.x,v.y,v.z);}
    function ValidateScriptScope() {
        if(!IsValid()) throw "invalid entity handle";
        local key="e"+id;
        if(!__EntityScopes.rawin(key)) {
            __EntityScopes[key] <- {self=this};
            __EntityScopes[key].setdelegate(getroottable());
        }
        return true;
    }
    function GetScriptScope() {ValidateScriptScope();return __EntityScopes["e"+id];}
    function Kill() {DoEntFire("!self","Kill","",0,this,this);}
    function ConnectOutput(output,functionName) {__host("ConnectOutput",id,output,functionName);}
    function DisconnectOutput(output,functionName) {__host("DisconnectOutput",id,output,functionName);}
}
// SpawnEntityFromTable(classname, keyvalues): the entity takes the next
// world index (reserved here, created by the host's "!spawn" command in the
// same order), is visible to Entities.Find* at once and returns its handle.
// Key/values go as key\x1fvalue records separated by \x1e; "connections"
// is { Output = { cmd1 = "target\x1binput\x1bparam\x1bdelay\x1btimes" } }.
__EntitySpawnNext <- 0;
function __KVVector(v) {
    if (typeof v == "instance") return Vector(v.x, v.y, v.z);
    local p = split(v.tostring(), " ");
    return p.len() >= 3 ? Vector(p[0].tofloat(), p[1].tofloat(), p[2].tofloat()) : Vector(0, 0, 0);
}
function SpawnEntityFromTable(classname, kv) {
    local id = __EntitySpawnNext;
    __EntitySpawnNext++;
    local records = [];
    foreach (key, value in kv) {
        if (key == "connections") {
            foreach (output, cmds in value) {
                foreach (_, cmd in cmds) records.append(output + "\x1f" + cmd);
            }
        } else if (typeof value == "instance") {
            records.append(key + "\x1f" + value.x + " " + value.y + " " + value.z);
        } else {
            records.append(key + "\x1f" + value);
        }
    }
    local payload = "";
    foreach (i, r in records) payload += (i > 0 ? "\x1e" : "") + r;
    __EntityData[id] <- {
        name = ("targetname" in kv) ? kv.targetname.tostring() : "",
        classname = classname,
        model = ("model" in kv) ? kv.model.tostring() : "",
        alive = true,
        origin = ("origin" in kv) ? __KVVector(kv.origin) : Vector(0, 0, 0),
        angles = ("angles" in kv) ? __KVVector(kv.angles) : Vector(0, 0, 0)
    };
    DoEntFire("!spawn", classname, id + "\x1d" + payload, 0, null, null);
    return __EntityHandle(id);
}
function __EntityHandle(id) {
    if(!__EntityHandles.rawin(id)) __EntityHandles[id] <- CBaseEntity(id);
    return __EntityHandles[id];
}
function __MatchEntityPattern(value,pattern) {
    value=value.tolower();pattern=pattern.tolower();
    local i=0,j=0,star=-1,retry=0;
    while(i<value.len()) {
        if(j<pattern.len() && pattern[j]==value[i]) {i++;j++;}
        else if(j<pattern.len() && pattern[j]=='*') {star=j++;retry=i;}
        else if(star>=0) {j=star+1;i=++retry;}
        else return false;
    }
    while(j<pattern.len() && pattern[j]=='*') j++;
    return j==pattern.len();
}
function __EntityFind(previous,value,field,origin=null,radius=null,nearest=false) {
    local after=previous==null?-1:previous.id;
    local best=null,distance=radius==null?null:radius*radius;
    foreach(id,data in __EntityData) {
        if(id<=after || !data.alive) continue;
        local actual=data[field];
        if(!__MatchEntityPattern(actual,value)) continue;
        local d=origin==null?0.0:(data.origin-origin).LengthSqr();
        if(distance!=null && d>distance) continue;
        if(best==null || (nearest && d<distance) || (!nearest && id<best)) {
            best=id;
            if(nearest) distance=d;
        }
    }
    return best==null?null:__EntityHandle(best);
}
Entities <- {
    function FindByName(previous,name) {return __EntityFind(previous,name,"name");}
    function FindByClassname(previous,classname) {return __EntityFind(previous,classname,"classname");}
    function First() {return __EntityFind(null,"*","classname");}
    function Next(previous) {return __EntityFind(previous,"*","classname");}
    function FindByModel(previous,model) {return __EntityFind(previous,model,"model");}
    function FindByClassnameWithin(previous,classname,origin,radius) {return __EntityFind(previous,classname,"classname",origin,radius);}
    function FindByNameWithin(previous,name,origin,radius) {return __EntityFind(previous,name,"name",origin,radius);}
    function FindByClassnameNearest(classname,origin,radius) {return __EntityFind(null,classname,"classname",origin,radius,true);}
    function FindByNameNearest(name,origin,radius) {return __EntityFind(null,name,"name",origin,radius,true);}
};
__OutputsPattern <- regexp("^On.*Output$");
function ConnectOutputs(scope) {
    foreach(key,value in scope) {
        if(typeof value=="function" && __OutputsPattern.match(key)) scope.self.ConnectOutput(key.slice(0,key.len()-6),key);
    }
}
DirectorScript.GetDirectorOptions <- function() {
    local options=DirectorScript.DirectorOptions;
    foreach(scope in [DirectorScript.MapScript,DirectorScript.MapScript.LocalScript,DirectorScript.MapScript.ChallengeScript]) {
        if(scope.rawin("DirectorOptions")) {
            scope.DirectorOptions.setdelegate(options);
            options=scope.DirectorOptions;
        }
    }
    return options;
};
GameEventCallbacks <- {};
ScriptEventCallbacks <- {};
__EngineEventListeners <- {};
function RegisterScriptGameEventListener(name) {__EngineEventListeners[name] <- true;}
function ClearGameEventCallbacks() {::GameEventCallbacks <- {};::ScriptEventCallbacks <- {};}
function __CollectEventCallbacks(scope,prefix,tableName,register) {
    if(typeof scope!="table") return;
    if(!(tableName in getroottable())) getroottable()[tableName] <- {};
    local handlers=getroottable()[tableName];
    foreach(key,value in scope) {
        if(typeof value!="function" || typeof key!="string" || key.find(prefix)!=0) continue;
        local event=key.slice(prefix.len());
        if(event.len()==0) continue;
        if(!(event in handlers)) {
            handlers[event] <- [];
            if(register) register(event);
        }
        else if(handlers[event].find(scope)!=null) continue;
        handlers[event].append(scope.weakref());
    }
}
function __CollectGameEventCallbacks(scope) {
    __CollectEventCallbacks(scope,"OnGameEvent_","GameEventCallbacks",::RegisterScriptGameEventListener);
    __CollectEventCallbacks(scope,"OnScriptEvent_","ScriptEventCallbacks",null);
}
function __RunEventCallbacks(event,params,prefix,tableName,warn) {
    local handlers=getroottable()[tableName];
    if(!(event in handlers)) return;
    for(local i=handlers[event].len()-1;i>=0;--i) {
        local scope=handlers[event][i];
        if(scope==null) handlers[event].remove(i);
        else scope[prefix+event](params);
    }
}
function __RunGameEventCallbacks(event,params) {__RunEventCallbacks(event,params,"OnGameEvent_","GameEventCallbacks",true);}
function FireScriptEvent(event,params) {__RunEventCallbacks(event,params,"OnScriptEvent_","ScriptEventCallbacks",false);}
function __DispatchGameEvent(event,params) {__RunGameEventCallbacks(event,params);}
function __UnregisterScope(scope) {
    foreach(handlers in [GameEventCallbacks,ScriptEventCallbacks]) {
        foreach(event,list in handlers) {
            for(local i=list.len()-1;i>=0;--i) if(list[i]==null || list[i]==scope) list.remove(i);
        }
    }
}
