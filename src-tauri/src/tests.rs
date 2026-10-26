mod auth;
mod bundle;
mod cli;
mod extensions;
mod frontend;
mod install;
mod ipc_state;
mod presentation;

fn frontend_source() -> String {
    [
        include_str!("../ui/index.html"),
        include_str!("../ui/js/runtime.js"),
        include_str!("../ui/js/settings.js"),
        include_str!("../ui/js/extensions.js"),
        include_str!("../ui/js/home.js"),
        include_str!("../ui/js/main.js"),
    ]
    .join("\n")
    .replace("\r\n", "\n")
}
