//! The stored TLS generation: certificate DER, SPKI SHA-256, key epoch and
//! endpoint manifest, written atomically to a user-only directory, plus
//! rotation and expiry warning (docs/23-mobile-m2.md §2, TM-01). Filled by
//! card M2-P1b.
