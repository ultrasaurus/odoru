use dl::extract;

#[test]
fn test_extract_davidtemkin() {
    let html = include_str!("fixtures/davidtemkin.html");
    let article = extract(html, "https://davidtemkin.com/about-mesa/")
        .expect("extraction should succeed");

    assert!(article.title.is_some());
    assert!(!article.content.is_empty());
}

#[test]
fn test_extract_ultrasaurus() {
    let html = include_str!("fixtures/ultrasaurus.html");
    let article = extract(html, "https://ultrasaurus.com/2019/04/when-reality-is-broken-change-the-rules/")
        .expect("extraction should succeed");

    assert_eq!(article.title.as_deref(), Some("when reality is broken, change the rules"));
    assert!(!article.content.is_empty());
    assert!(article.authors.contains(&"Sarah".to_string()));
    assert!(article.date.as_deref() == Some("2019-04-28"));
}

#[test]
fn test_extract_plain_text() {
    let html = include_str!("fixtures/davidtemkin.html");
    let article = extract(html, "https://davidtemkin.com/about-mesa/")
        .expect("extraction should succeed");

    assert!(!article.plain_text.is_empty());
    // plain text should not contain markdown headings
    assert!(!article.plain_text.contains("# "));
}