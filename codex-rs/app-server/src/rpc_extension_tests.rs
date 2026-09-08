use super::*;
use pretty_assertions::assert_eq;

#[derive(Debug)]
struct StubExtension {
    methods: &'static [&'static str],
}

impl AppServerRpcExtension for StubExtension {
    fn methods(&self) -> &'static [&'static str] {
        self.methods
    }

    fn handle<'a>(
        &'a self,
        _context: AppServerRpcContext,
        _method: &'a str,
        _params: Option<serde_json::Value>,
    ) -> AppServerRpcFuture<'a> {
        Box::pin(async { panic!("registration must not execute a handler") })
    }
}

#[test]
fn registry_rejects_native_invalid_and_duplicate_methods() {
    for (methods, expected) in [
        (
            &["turn/start"][..],
            AppServerRpcRegistryError::NativeMethod("turn/start".into()),
        ),
        (
            &["thread/start"][..],
            AppServerRpcRegistryError::NativeMethod("thread/start".into()),
        ),
        (
            &[""][..],
            AppServerRpcRegistryError::InvalidMethod(String::new()),
        ),
        (
            &["unqualified"][..],
            AppServerRpcRegistryError::InvalidMethod("unqualified".into()),
        ),
        (
            &["catalyst/read", "catalyst/read"][..],
            AppServerRpcRegistryError::DuplicateMethod("catalyst/read".into()),
        ),
    ] {
        let result = AppServerRpcRegistry::new(vec![Arc::new(StubExtension { methods })]);
        assert_eq!(result.unwrap_err(), expected);
    }
    let result = AppServerRpcRegistry::new(vec![
        Arc::new(StubExtension {
            methods: &["catalyst/read"],
        }),
        Arc::new(StubExtension {
            methods: &["catalyst/read"],
        }),
    ]);
    assert_eq!(
        result.unwrap_err(),
        AppServerRpcRegistryError::DuplicateMethod("catalyst/read".into())
    );
}

#[test]
fn registry_routes_exact_methods_and_distinguishes_namespace_boundaries() {
    let first: Arc<dyn AppServerRpcExtension> = Arc::new(StubExtension {
        methods: &["catalyst/account/read"],
    });
    let second: Arc<dyn AppServerRpcExtension> = Arc::new(StubExtension {
        methods: &["other/read"],
    });
    let registry =
        AppServerRpcRegistry::new(vec![Arc::clone(&first), Arc::clone(&second)]).unwrap();
    assert!(Arc::ptr_eq(
        registry.get("catalyst/account/read").unwrap(),
        &first
    ));
    assert!(Arc::ptr_eq(registry.get("other/read").unwrap(), &second));
    assert!(registry.get("catalyst/account/missing").is_none());
    assert!(registry.contains_namespace("catalyst/account/missing"));
    assert!(!registry.contains_namespace("catalyst_extra/read"));
    assert!(!registry.contains_namespace("catalyst"));
    assert!(!registry.contains_namespace("turn/start"));
}
