#[path = "../../assets/windows/resources.rs"]
mod resources;

fn main() {
    resources::compile(&["pod-hook"]);
}
