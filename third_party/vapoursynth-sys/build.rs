fn main() {
    println!("cargo:rerun-if-changed=headers/VapourSynth4.h");
    println!("cargo:rerun-if-changed=headers/VSScript4.h");

    #[cfg(feature = "bindgen")]
    {
        generate_bindings();
    }
}

#[cfg(feature = "bindgen")]
fn generate_bindings() {
    use std::env;
    use std::path::PathBuf;

    // Generate bindings
    let bindings = bindgen::Builder::default()
        .header("headers/wrapper.h")
        .blocklist_function("getVSScriptAPI") // VSScript is expected to be dynamically loaded
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .expect("Unable to generate bindings");

    // Write the bindings to the $OUT_DIR/bindings.rs file
    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("Couldn't write bindings!");
}
