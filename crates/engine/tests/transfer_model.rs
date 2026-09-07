use excalibur_engine::transfer::{Direction, Endpoint, Transfer};

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
