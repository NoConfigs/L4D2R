// Squirrel embedding bridge. All access is serialized by Rust's VM mutex.
#include <squirrel.h>
#include <sqstdmath.h>
#include <sqstdstring.h>
#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstdarg>
#include <cstdio>
#include <string>
#include <unordered_map>
#include <vector>

struct Command { std::string target, input, param; float delay; int activator=-1, caller=-1; };
struct Bridge {
    HSQUIRRELVM vm;
    std::unordered_map<std::string, std::string> sources;
    std::unordered_map<int, HSQOBJECT> scopes;
    std::unordered_map<std::string, double> state;
    std::unordered_map<std::string, std::string> strings;
    std::vector<std::string> includes;
    std::vector<Command> commands;
    std::vector<std::pair<std::string, double>> options;
    std::string error, log;
    int budget = 1000000;
    double (*random)(void*,int,double,double) = nullptr;
    void* random_context = nullptr;
    std::chrono::steady_clock::time_point deadline;
};
static Bridge* host(HSQUIRRELVM v) { return static_cast<Bridge*>(sq_getforeignptr(v)); }
// Called on every VM instruction, including loops without line/debug events.
bool l4d2r_sq_budget(HSQUIRRELVM v) {
    Bridge* b = host(v);
    if (!b) return false;
    if (--b->budget < 0) return true;
    return (b->budget & 1023) == 0 && std::chrono::steady_clock::now() > b->deadline;
}
static void budget(Bridge* b) {
    b->budget = 1000000;
    b->deadline = std::chrono::steady_clock::now() + std::chrono::milliseconds(20);
    b->error.clear();
}
static std::string string_at(HSQUIRRELVM v, SQInteger i) {
    if (sq_gettype(v, i) == OT_NULL) return "";
    const SQChar* s = nullptr;
    if (SQ_SUCCEEDED(sq_getstring(v, i, &s))) return s;
    SQInteger top = sq_gettop(v);
    std::string result;
    if (SQ_SUCCEEDED(sq_tostring(v, i)) && SQ_SUCCEEDED(sq_getstring(v, -1, &s))) result = s;
    sq_settop(v, top);
    return result;
}
static void remember_error(Bridge* b) {
    sq_getlasterror(b->vm);
    std::string message = string_at(b->vm, -1);
    sq_pop(b->vm, 1);
    if (b->error.empty() && !message.empty()) b->error = message;
    if (b->error.empty()) b->error = "Squirrel execution failed";
}
static void compiler_error(HSQUIRRELVM v, const SQChar* desc, const SQChar* src, SQInteger line, SQInteger column) {
    host(v)->error = std::string(src) + ":" + std::to_string(line) + ":" + std::to_string(column) + ": " + desc;
}
static void print(HSQUIRRELVM v, const SQChar* fmt, ...) {
    char buffer[2048]; va_list args; va_start(args, fmt); vsnprintf(buffer, sizeof(buffer), fmt, args); va_end(args);
    Bridge* b = host(v);
    if (b->log.size() < 65536) b->log += buffer;
}
static bool execute(Bridge* b, HSQOBJECT scope, const std::string& name, const std::string& source) {
    HSQUIRRELVM v = b->vm;
    SQInteger top = sq_gettop(v);
    bool ok = SQ_SUCCEEDED(sq_compilebuffer(v, source.data(), source.size(), name.c_str(), SQTrue));
    if (ok) {
        sq_pushobject(v, scope);
        ok = SQ_SUCCEEDED(sq_call(v, 1, SQFalse, SQTrue));
    }
    if (!ok) remember_error(b);
    sq_settop(v, top);
    return ok;
}
static std::string normalize(std::string s) {
    std::replace(s.begin(), s.end(), '\\', '/');
    std::transform(s.begin(), s.end(), s.begin(), [](unsigned char c){return char(std::tolower(c));});
    if (s.compare(0, 17, "scripts/vscripts/") == 0) s.erase(0,17);
    if (s.size() > 4 && (s.substr(s.size()-4) == ".nut" || s.substr(s.size()-4) == ".nuc")) s.resize(s.size()-4);
    return s;
}
static bool run(Bridge* b, HSQOBJECT scope, std::string name) {
    name = normalize(name);
    auto source = b->sources.find(name);
    if (source == b->sources.end()) { b->error = "missing script: " + name; return false; }
    if (b->includes.size() >= 32 || std::find(b->includes.begin(), b->includes.end(), name) != b->includes.end()) {
        b->error = "include cycle/depth: " + name; return false;
    }
    b->includes.push_back(name);
    bool ok = execute(b, scope, name, source->second);
    b->includes.pop_back();
    return ok;
}
static SQInteger include_script(HSQUIRRELVM v) {
    Bridge* b = host(v);
    HSQOBJECT scope; sq_getstackobj(v, 1, &scope);
    if (sq_gettop(v) >= 3 && sq_gettype(v,3) == OT_TABLE) sq_getstackobj(v,3,&scope);
    if (!run(b, scope, string_at(v,2))) return sq_throwerror(v,b->error.c_str());
    sq_pushbool(v, SQTrue); return 1;
}
static SQInteger engine_call(HSQUIRRELVM v) {
    Bridge* b = host(v);
    std::string name = string_at(v, 2);
    auto num = b->state.find(name);
    if (num != b->state.end()) { sq_pushfloat(v, SQFloat(num->second)); return 1; }
    auto str = b->strings.find(name);
    if (str != b->strings.end()) { sq_pushstring(v, str->second.c_str(), -1); return 1; }
    if(name=="ConvarGet" || name=="ConvarSet") {
        std::string key=string_at(v,3);auto found=b->state.find("Convar."+key);
        if(found==b->state.end()) return sq_throwerror(v,("unbound convar: "+key).c_str());
        if(name=="ConvarGet") {sq_pushfloat(v,SQFloat(found->second));return 1;}
        SQFloat value;
        if(SQ_FAILED(sq_getfloat(v,4,&value)) || !std::isfinite(value) || value<0) return sq_throwerror(v,"convar value must be finite and nonnegative");
        found->second=double(value);Command command;command.target="!convar";command.input=key;command.param=std::to_string(double(value));b->commands.push_back(command);return 0;
    }
    if (name == "RandomInt" || name == "RandomFloat") {
        SQFloat lo=0,hi=1; sq_getfloat(v,3,&lo); sq_getfloat(v,4,&hi);
        if(!b->random) return sq_throwerror(v,"random stream is not installed");
        if (name == "RandomInt") {
            SQInteger a=0,z=0;
            if(SQ_FAILED(sq_getinteger(v,3,&a)) || SQ_FAILED(sq_getinteger(v,4,&z))) return sq_throwerror(v,"invalid random integer range");
            sq_pushinteger(v,SQInteger(b->random(b->random_context,1,double(a),double(z))));
        } else {
            sq_pushfloat(v,SQFloat(b->random(b->random_context,0,lo,hi)));
        }
        return 1;
    }
    if (name == "EntFire" || name == "DirectorCommand" || name == "Sound" || name=="Think" || name=="ConnectOutput" || name=="DisconnectOutput") {
        if (b->commands.size() >= 128) return sq_throwerror(v,"engine command budget exhausted");
        Command c;
        c.target = name == "EntFire" ? string_at(v,3) : name == "Sound" ? "!sound" : name=="Think"?"!think":name=="ConnectOutput"?"!connect":name=="DisconnectOutput"?"!disconnect":"!director";
        c.input = name == "EntFire" ? string_at(v,4) : string_at(v,3);
        c.param = name == "EntFire" ? string_at(v,5) : string_at(v,4);
        if(name=="ConnectOutput" || name=="DisconnectOutput") { c.input=string_at(v,3)+" "+string_at(v,4);c.param=string_at(v,5); }
        c.delay = 0;
        if (name == "EntFire" && sq_gettop(v) >= 6) sq_getfloat(v,6,&c.delay);
        if (!std::isfinite(c.delay)) return sq_throwerror(v,"invalid EntFire delay");
        c.delay = std::max(0.f,c.delay);
        if(name=="EntFire") {
            SQInteger id;
            if(sq_gettop(v)>=7 && SQ_SUCCEEDED(sq_getinteger(v,7,&id))) c.activator=int(id);
            if(sq_gettop(v)>=8 && SQ_SUCCEEDED(sq_getinteger(v,8,&id))) c.caller=int(id);
        }
        b->commands.push_back(c); return 0;
    }
    return sq_throwerror(v, ("unbound engine API: " + name).c_str());
}
static void closure(Bridge* b, const char* name, SQFUNCTION f) {
    sq_pushroottable(b->vm); sq_pushstring(b->vm,name,-1); sq_newclosure(b->vm,f,0); sq_newslot(b->vm,-3,SQFalse); sq_pop(b->vm,1);
}
static bool get_scope(Bridge* b, int id, const char* path) {
    HSQUIRRELVM v = b->vm; SQInteger top = sq_gettop(v); sq_pushroottable(v);
    std::string p(path); size_t at=0;
    while (at<p.size()) {
        size_t end=p.find('.',at); if(end==std::string::npos) end=p.size();
        std::string key=p.substr(at,end-at); sq_pushstring(v,key.c_str(),-1);
        if(SQ_FAILED(sq_get(v,-2))) { sq_settop(v,top); return false; }
        sq_remove(v,-2); at=end+1;
    }
    auto old=b->scopes.find(id); if(old!=b->scopes.end()) sq_release(v,&old->second);
    HSQOBJECT scope; sq_getstackobj(v,-1,&scope); sq_addref(v,&scope); b->scopes[id]=scope; sq_settop(v,top); return true;
}
static bool call(Bridge* b,int id,const char* table,const char* function,double* result=nullptr) {
    auto found=b->scopes.find(id); if(found==b->scopes.end()) { b->error="missing script scope";return false; }
    HSQUIRRELVM v=b->vm; SQInteger top=sq_gettop(v);
    sq_pushobject(v,found->second);
    if(table && *table) { sq_pushstring(v,table,-1); if(SQ_FAILED(sq_rawget(v,-2))) { sq_settop(v,top);return true; } sq_remove(v,-2); }
    HSQOBJECT receiver; sq_getstackobj(v,-1,&receiver);
    sq_pushstring(v,function,-1);
    if(SQ_FAILED(sq_rawget(v,-2))) { if(result) b->error="missing think function: "+std::string(function);sq_settop(v,top);return !result; }
    if(sq_gettype(v,-1)!=OT_CLOSURE && sq_gettype(v,-1)!=OT_NATIVECLOSURE) { b->error="callback is not callable";sq_settop(v,top);return false; }
    sq_pushobject(v,receiver);
    bool ok=SQ_SUCCEEDED(sq_call(v,1,result?SQTrue:SQFalse,SQTrue));
    if(!ok) remember_error(b);
    else if(result) {SQFloat interval;if(SQ_SUCCEEDED(sq_getfloat(v,-1,&interval)) && std::isfinite(interval)) *result=interval;else *result=0.1;}
    sq_settop(v,top);return ok;
}
extern "C" {
void* rvm_new() {
    Bridge* b=new Bridge(); b->vm=sq_open(1024); sq_setforeignptr(b->vm,b); sq_setprintfunc(b->vm,print,print); sq_setcompilererrorhandler(b->vm,compiler_error);
    sq_pushroottable(b->vm); sqstd_register_mathlib(b->vm); sqstd_register_stringlib(b->vm); sq_pop(b->vm,1);
    closure(b,"__host",engine_call); closure(b,"DoIncludeScript",include_script);
    get_scope(b,0,""); budget(b); return b;
}
void rvm_free(void* ptr) { Bridge* b=(Bridge*)ptr; for(auto &s:b->scopes) sq_release(b->vm,&s.second); sq_close(b->vm); delete b; }
void rvm_source(void* ptr,const char* name,const char* code) { ((Bridge*)ptr)->sources[normalize(name)]=code; }
int rvm_eval(void* ptr,int scope,const char* name,const char* code) { Bridge* b=(Bridge*)ptr; budget(b); auto s=b->scopes.find(scope); if(s==b->scopes.end()) { b->error="missing script scope";return 0; } return execute(b,s->second,name,code); }
int rvm_run(void* ptr,int scope,const char* name) { Bridge* b=(Bridge*)ptr; budget(b);auto s=b->scopes.find(scope);if(s==b->scopes.end()) { b->error="missing script scope";return 0; } return run(b,s->second,name); }
int rvm_compile(void* ptr,const char* name,const char* code) {
    Bridge* b=(Bridge*)ptr;budget(b);SQInteger top=sq_gettop(b->vm);
    bool ok=SQ_SUCCEEDED(sq_compilebuffer(b->vm,code,std::char_traits<char>::length(code),name,SQTrue));
    if(!ok) remember_error(b);sq_settop(b->vm,top);return ok;
}
int rvm_scope(void* ptr,int id,const char* path) { return get_scope((Bridge*)ptr,id,path); }
void rvm_release_scope(void* ptr,int id) {Bridge* b=(Bridge*)ptr;auto s=b->scopes.find(id);if(s!=b->scopes.end()) {sq_release(b->vm,&s->second);b->scopes.erase(s);}}
int rvm_call(void* ptr,int id,const char* table,const char* function) { Bridge* b=(Bridge*)ptr;budget(b);return call(b,id,table,function); }
int rvm_think(void* ptr,int id,const char* function,double* result) {Bridge* b=(Bridge*)ptr;budget(b);return call(b,id,"",function,result);}
void rvm_number(void* ptr,const char* key,double value) { ((Bridge*)ptr)->state[key]=value; }
void rvm_random(void* ptr,double (*function)(void*,int,double,double),void* context) { Bridge* b=(Bridge*)ptr;b->random=function;b->random_context=context; }
void rvm_string(void* ptr,const char* key,const char* value) { ((Bridge*)ptr)->strings[key]=value; }
const char* rvm_error(void* ptr) { return ((Bridge*)ptr)->error.c_str(); }
const char* rvm_log(void* ptr) { return ((Bridge*)ptr)->log.c_str(); }
void rvm_clear_log(void* ptr) { ((Bridge*)ptr)->log.clear(); }
int rvm_options(void* ptr) {
    Bridge* b=(Bridge*)ptr;budget(b);b->options.clear();
    if(!execute(b,b->scopes[0],"options","__current_options <- DirectorScript.GetDirectorOptions();")) return -1;
    HSQUIRRELVM v=b->vm; SQInteger top=sq_gettop(v);sq_pushroottable(v);sq_pushstring(v,"__current_options",-1);
    if(SQ_FAILED(sq_get(v,-2)) || sq_gettype(v,-1)!=OT_TABLE) { sq_settop(v,top);return 0; }
    sq_remove(v,-2);
    for(int depth=0;depth<16;depth++) {
        sq_pushnull(v);
        while(SQ_SUCCEEDED(sq_next(v,-2))) {
            const SQChar* key=nullptr;SQFloat value;
            if(SQ_SUCCEEDED(sq_getstring(v,-2,&key))) {
                bool valid=SQ_SUCCEEDED(sq_getfloat(v,-1,&value));
                if(sq_gettype(v,-1)==OT_BOOL) { SQBool bv; sq_getbool(v,-1,&bv);value=bv?1.f:0.f;valid=true; }
                // First occurrence wins, including nonnumeric child overrides.
                auto old=std::find_if(b->options.begin(),b->options.end(),[key](const std::pair<std::string,double>& p){return p.first==key;});
                if(old==b->options.end()) b->options.push_back({key,valid&&std::isfinite(value)?double(value):NAN});
            }
            sq_pop(v,2);
        }
        sq_pop(v,1);
        if(SQ_FAILED(sq_getdelegate(v,-1)) || sq_gettype(v,-1)==OT_NULL) break;
        sq_remove(v,-2);
    }
    sq_settop(v,top);
    b->options.erase(std::remove_if(b->options.begin(),b->options.end(),[](const std::pair<std::string,double>& p){return !std::isfinite(p.second);}),b->options.end());
    return int(b->options.size());
}
const char* rvm_option_key(void* ptr,int index) { return ((Bridge*)ptr)->options.at(index).first.c_str(); }
double rvm_option_value(void* ptr,int index) { return ((Bridge*)ptr)->options.at(index).second; }
// Resolve through the same current option table/delegate chain as numeric
// Director values. Missing callback means the native default policy applies.
int rvm_option_bool_call(void* ptr,const char* name,const char* argument,int* result) {
    Bridge* b=(Bridge*)ptr;budget(b);
    if(!execute(b,b->scopes[0],"policy options","__current_options <- DirectorScript.GetDirectorOptions();")) return -1;
    HSQUIRRELVM v=b->vm;SQInteger top=sq_gettop(v);
    sq_pushroottable(v);sq_pushstring(v,"__current_options",-1);
    if(SQ_FAILED(sq_get(v,-2)) || sq_gettype(v,-1)!=OT_TABLE) {sq_settop(v,top);return 0;}
    sq_remove(v,-2);HSQOBJECT receiver;sq_getstackobj(v,-1,&receiver);
    sq_pushstring(v,name,-1);
    if(SQ_FAILED(sq_get(v,-2))) {sq_settop(v,top);return 0;}
    if(sq_gettype(v,-1)!=OT_CLOSURE && sq_gettype(v,-1)!=OT_NATIVECLOSURE) {
        b->error="Director policy is not callable: "+std::string(name);sq_settop(v,top);return -1;
    }
    sq_pushobject(v,receiver);sq_pushstring(v,argument,-1);
    if(SQ_FAILED(sq_call(v,2,SQTrue,SQTrue))) {remember_error(b);sq_settop(v,top);return -1;}
    if(sq_gettype(v,-1)!=OT_BOOL) {b->error="Director policy must return boolean: "+std::string(name);sq_settop(v,top);return -1;}
    SQBool value;sq_getbool(v,-1,&value);*result=value?1:0;sq_settop(v,top);return 1;
}
int rvm_commands(void* ptr) { return int(((Bridge*)ptr)->commands.size()); }
const char* rvm_command_text(void* ptr,int index,int field) { Command& c=((Bridge*)ptr)->commands.at(index);return field==0?c.target.c_str():field==1?c.input.c_str():c.param.c_str(); }
float rvm_command_delay(void* ptr,int index) { return ((Bridge*)ptr)->commands.at(index).delay; }
int rvm_command_context(void* ptr,int index,int field) {Command& c=((Bridge*)ptr)->commands.at(index);return field==0?c.activator:c.caller;}
void rvm_clear_commands(void* ptr) { ((Bridge*)ptr)->commands.clear(); }
}
