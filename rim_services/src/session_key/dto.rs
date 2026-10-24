/// The reload keys `reloadSession` hands a tab that reloaded: the one its stored data is
/// sealed under, and the one to seal it under next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReloadKeysDto {
    pub reload_key: Vec<u8>,
    pub next_reload_key: Vec<u8>,
}
