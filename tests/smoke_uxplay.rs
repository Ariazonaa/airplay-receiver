//! Port von test_smoke.cpp. Beweist auch, dass UxPlay die Rust-`dnssd_*` beim Linken findet.
use airplay::ffi::uxplay;

#[test]
fn uxplay_core_links() {
    // SAFETY: leere Callbacks sind für raop_init zulässig (die Struktur wird nur kopiert).
    unsafe {
        let mut callbacks: uxplay::raop_callbacks_t = std::mem::zeroed();
        let raop = uxplay::raop_init(&mut callbacks);
        // Mit leeren Callbacks darf raop_init ablehnen; es darf nur nicht abstürzen.
        if !raop.is_null() {
            uxplay::raop_destroy(raop);
        }
    }
}
