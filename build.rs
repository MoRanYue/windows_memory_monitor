fn main() {
    slint_build::compile("ui/appwindow.slint").expect("Slint build failed");

    // 仅 Windows 目标时将 icon.ico 作为资源嵌入 EXE（标题栏/任务栏/资源管理器图标）
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        if matches!(
            embed_resource::compile("assets/icon.rc", embed_resource::NONE),
            embed_resource::CompilationResult::Failed(_)
        ) {
            panic!("embed-resource: failed to compile assets/icon.rc");
        }
    }
}
