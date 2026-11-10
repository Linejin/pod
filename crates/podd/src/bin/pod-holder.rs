//! `pod-holder`: keeps one terminal alive independently of the daemon. Started by `podd`.

fn main() {
    std::process::exit(podd::holder::main());
}
