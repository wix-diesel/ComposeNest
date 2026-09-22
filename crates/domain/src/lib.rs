//! Domain values and invariants for ComposeNest.

/// Returns the application title displayed by supported clients.
#[must_use]
pub const fn application_title() -> &'static str {
    "ComposeNest"
}

#[cfg(test)]
mod tests {
    use super::application_title;

    #[test]
    fn returns_the_product_name() {
        assert_eq!(application_title(), "ComposeNest");
    }
}
