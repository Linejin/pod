//! T1 fills this lifecycle after P1/P2/P3. No socket or generic RPC adapter.
use super::auth::AuthenticatedDevice;
use crate::protocol::mobile_v1 as wire;
#[derive(Default)]
pub struct Listener;
impl Listener {
    pub fn is_enabled(&self) -> bool {
        false
    }
    pub fn enable(&mut self) -> Result<(), wire::Error> {
        Err(wire::Error::refusal())
    }
    pub fn call(
        &self,
        _ctx: &AuthenticatedDevice,
        _request: wire::Call,
    ) -> Result<wire::Reply, wire::Error> {
        Err(wire::Error::refusal())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn listener_cannot_enable() {
        let mut listener = Listener;
        assert!(!listener.is_enabled());
        assert_eq!(listener.enable(), Err(wire::Error::refusal()));
        assert!(!listener.is_enabled());
    }
}
