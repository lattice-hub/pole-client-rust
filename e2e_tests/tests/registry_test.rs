use e2e_tests::cases::{default_cases, select_cases};

#[test]
fn default_registry_covers_all_required_capabilities() {
    let names: Vec<&str> = default_cases().iter().map(|case| case.name()).collect();

    assert_eq!(
        names,
        vec![
            "connectivity",
            "service-discovery",
            "config-center",
            "routing",
            "ratelimit",
            "circuitbreaker",
            "lane-routing",
            "fault-detect",
            "lossless",
            "mirror",
            "auth-security",
            "mock",
        ]
    );
}

#[test]
fn selected_cases_keep_registry_order_and_reject_unknown_names() {
    let selected = select_cases(
        &default_cases(),
        &["mock".to_string(), "routing".to_string()],
    )
    .expect("known cases should be selected");

    assert_eq!(
        selected.iter().map(|case| case.name()).collect::<Vec<_>>(),
        vec!["routing", "mock"]
    );

    let err = select_cases(&default_cases(), &["unknown".to_string()]).unwrap_err();
    assert!(err.to_string().contains("unknown e2e case: unknown"));
}
