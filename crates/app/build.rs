fn main() {
    let config = slint_build::CompilerConfiguration::new()
        .with_style("fluent-dark".into())
        .with_include_paths(vec!["../../ui".into()]);

    slint_build::compile_with_config("../../ui/app.slint", config)
        .expect("failed to compile Slint sources");

    println!("cargo:rerun-if-changed=../../ui");
}
