fn main() {
    println!("cargo:rerun-if-changed=vendor/squirrel");
    println!("cargo:rerun-if-changed=src/vscript/bridge.cpp");
    let mut build = cc::Build::new();
    build
        .cpp(true)
        .warnings(false)
        .define("L4D2R_SQ32_VALUES", None)
        .include("vendor/squirrel/include")
        .include("vendor/squirrel/squirrel")
        .include("vendor/squirrel/sqstdlib")
        .flag_if_supported("/EHsc")
        .flag_if_supported("-std=c++11")
        .file("src/vscript/bridge.cpp");
    for entry in std::fs::read_dir("vendor/squirrel/squirrel").unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|s| s == "cpp") {
            build.file(path);
        }
    }
    for file in ["sqstdmath.cpp", "sqstdstring.cpp", "sqstdrex.cpp"] {
        build.file(format!("vendor/squirrel/sqstdlib/{file}"));
    }
    build.compile("l4d2r_squirrel");
}
