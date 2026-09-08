use excalibur_engine::transfer::{Direction, Endpoint, HttpDownloadOptions, Transfer};

#[test]
fn legacy_rows_load_and_lazily_become_share_endpoints() {
    let row = serde_json::json!({
        "id": "old",
        "direction": "down",
        "name": "file.bin",
        "sourcePath": "remote/file.bin",
        "destPath": "/tmp/file.bin",
        "bytesDone": 0,
        "bytesTotal": 12,
        "rate": null,
        "etaSeconds": null,
        "status": "queued",
        "error": null,
        "queuedAtMs": 1
    });
    let transfer: Transfer = serde_json::from_value(row).expect("legacy row");
    assert_eq!(transfer.direction, Some(Direction::Down));
    assert_eq!(
        transfer.from_endpoint(),
        Some(Endpoint::Share {
            mount: None,
            path: "remote/file.bin".to_owned()
        })
    );
    assert_eq!(
        transfer.to_endpoint(),
        Some(Endpoint::Local {
            path: "/tmp/file.bin".to_owned()
        })
    );
}

#[test]
fn legacy_upload_rows_upgrade_to_local_then_share_endpoints() {
    let row = serde_json::json!({
        "id": "old-up",
        "direction": "up",
        "mount": "office",
        "name": "notes.txt",
        "sourcePath": "/local/notes.txt",
        "destPath": "remote/notes.txt",
        "bytesDone": 0,
        "bytesTotal": 9,
        "rate": null,
        "etaSeconds": null,
        "status": "queued",
        "error": null,
        "queuedAtMs": 2
    });
    let transfer: Transfer = serde_json::from_value(row).expect("legacy row");
    assert_eq!(transfer.direction, Some(Direction::Up));
    assert_eq!(
        transfer.from_endpoint(),
        Some(Endpoint::Local {
            path: "/local/notes.txt".to_owned()
        })
    );
    assert_eq!(
        transfer.to_endpoint(),
        Some(Endpoint::Share {
            mount: Some("office".to_owned()),
            path: "remote/notes.txt".to_owned()
        })
    );
}

#[test]
fn endpoint_native_rows_round_trip_with_direction_absent() {
    let transfer =
        Transfer::http("download", "https://example.test/f", "/tmp/downloads").expect("valid URL");
    let text = serde_json::to_value(&transfer).expect("serialize");
    assert_eq!(text["direction"], serde_json::Value::Null);
    let back: Transfer = serde_json::from_value(text).expect("restore");
    assert_eq!(back, transfer);
    assert_eq!(back.direction, None);
}

#[test]
fn http_constructor_is_endpoint_native_and_uses_safe_url_name() {
    let transfer = Transfer::new_http(
        "download",
        "https://example.test/releases/archive.tar.gz",
        "/tmp/downloads",
        42,
    )
    .expect("valid URL");
    assert_eq!(transfer.direction, None);
    assert_eq!(transfer.mount, None);
    assert_eq!(transfer.name, "archive.tar.gz");
    assert_eq!(
        transfer.from,
        Some(Endpoint::Http {
            url: "https://example.test/releases/archive.tar.gz".to_owned()
        })
    );
    assert_eq!(
        transfer.to,
        Some(Endpoint::Local {
            path: "/tmp/downloads".to_owned()
        })
    );
}

#[test]
fn http_constructor_rejects_unsupported_or_credential_urls() {
    for url in [
        "ftp://example.test/file",
        "https://alice:secret@example.test/file",
        "https://example.test/file?token=secret",
    ] {
        assert!(Transfer::new_http("bad", url, "/tmp", 0).is_err(), "{url}");
    }
}

#[test]
fn http_controls_survive_the_queue_json_shape() {
    let transfer = Transfer::http_with_options(
        "controlled",
        "https://primary.test/file.iso",
        "/tmp/downloads",
        HttpDownloadOptions {
            connections: Some(3),
            mirrors: vec!["https://mirror.test/file.iso".to_owned()],
            preferred_mirror: Some(1),
            retry: Some(excalibur_engine::transfer::HttpRetryPolicy {
                max_attempts: 7,
                initial_wait_seconds: 2,
                max_wait_seconds: 45,
            }),
        },
    )
    .unwrap();
    let json = serde_json::to_value(&transfer).unwrap();
    assert_eq!(json["httpOptions"]["connections"], 3);
    assert_eq!(json["httpOptions"]["preferredMirror"], 1);
    assert_eq!(json["httpOptions"]["retry"]["maxAttempts"], 7);
    assert_eq!(serde_json::from_value::<Transfer>(json).unwrap(), transfer);
}

#[test]
fn http_controls_refuse_invalid_counts_and_mirror_selection() {
    for options in [
        HttpDownloadOptions {
            connections: Some(0),
            ..HttpDownloadOptions::default()
        },
        HttpDownloadOptions {
            mirrors: vec!["https://mirror.test/file.iso".to_owned()],
            preferred_mirror: Some(2),
            ..HttpDownloadOptions::default()
        },
        HttpDownloadOptions {
            retry: Some(excalibur_engine::transfer::HttpRetryPolicy {
                max_attempts: 0,
                initial_wait_seconds: 1,
                max_wait_seconds: 30,
            }),
            ..HttpDownloadOptions::default()
        },
        HttpDownloadOptions {
            retry: Some(excalibur_engine::transfer::HttpRetryPolicy {
                max_attempts: 3,
                initial_wait_seconds: 60,
                max_wait_seconds: 10,
            }),
            ..HttpDownloadOptions::default()
        },
    ] {
        assert!(Transfer::http_with_options(
            "invalid",
            "https://primary.test/file.iso",
            "/tmp/downloads",
            options,
        )
        .is_err());
    }
}
