use super::*;

#[test]
fn readiness_requires_runtime_and_styles_not_optional_fonts() {
    let loader = loader();
    let ready = loader
        .split("function ready()")
        .nth(1)
        .unwrap()
        .split("function failed()")
        .next()
        .unwrap();
    assert!(ready.contains("data-zxs-runtime"));
    assert!(!ready.contains("data-zxs-fonts"));
    assert!(loader.contains("const stylesheets = new Promise((resolve, reject) =>"));
    assert!(loader.contains("if (!settled) return module.default()"));
    assert!(loader.contains("setTimeout(() => fail(\"timeout\"), 30000)"));
}

#[test]
fn parser_installs_the_single_owner_before_creating_either_required_link() {
    let index = index();
    let boot = index.find("<script src=\"./boot.js\"></script>").unwrap();
    let startup = index
        .find("<link id=\"zxs-startup-style\" rel=\"stylesheet\" href=\"./styles/startup.css\">")
        .unwrap();
    let app = index
        .find("<link id=\"zxs-app-style\" rel=\"stylesheet\" href=\"./zxs.css\">")
        .unwrap();
    assert!(boot < startup && startup < app && app < index.find("</head>").unwrap());
    assert_eq!(index.matches("<script").count(), 1);
    assert_eq!(index.matches("rel=\"stylesheet\"").count(), 2);
    for forbidden in [
        " async",
        " defer",
        "type=\"module\"",
        " onload=",
        " onerror=",
    ] {
        assert!(!index.contains(forbidden), "{forbidden}");
    }
    let loader = loader();
    assert!(loader.contains("(() => {\n\"use strict\";"));
    assert!(loader.ends_with("})();\n"));
    assert!(loader.contains("if (html.hasAttribute(\"data-zxs-startup\")) throw new Error"));
}

#[test]
fn only_captured_load_events_admit_both_required_stylesheets() {
    let loader = loader();
    let stylesheets = loader
        .split("const stylesheets = new Promise")
        .nth(1)
        .unwrap()
        .split("// Evaluate bindings")
        .next()
        .unwrap();
    assert!(loader.contains("new Set([\"zxs-startup-style\", \"zxs-app-style\"])"));
    assert!(stylesheets.contains("if (!pendingStylesheets.has(link.id)) return;"));
    assert!(stylesheets.contains("if (event.type === \"error\")"));
    assert!(stylesheets.contains("reject(new Error(`Required stylesheet failed: ${link.href}`))"));
    assert!(stylesheets.contains("pendingStylesheets.delete(link.id)"));
    assert!(stylesheets.contains("if (pendingStylesheets.size === 0) { detach(); resolve(); }"));
    assert_eq!(stylesheets.matches("resolve();").count(), 1);
    for event in ["load", "error"] {
        assert!(stylesheets.contains(&format!(
            "document.addEventListener(\"{event}\", completed, true)"
        )));
        assert!(stylesheets.contains(&format!(
            "document.removeEventListener(\"{event}\", completed, true)"
        )));
    }
    assert!(loader.contains("for (const detach of stylesheetListeners.splice(0)) detach();"));
    for forbidden in [
        "if (link.sheet)",
        "cssRules",
        "getEntriesByType",
        "setInterval",
        "document.fonts",
    ] {
        assert!(!loader.contains(forbidden), "{forbidden}");
    }
}

#[test]
fn failure_releases_paint_blocking_without_restarting_stylesheet_requests() {
    let loader = loader();
    let render = loader
        .split("function renderFailure(stage)")
        .nth(1)
        .unwrap();
    assert!(render.contains(
        "for (const id of pendingStylesheets) document.getElementById(id).disabled = true;"
    ));
    let settle = loader
        .split("function settle(next)")
        .nth(1)
        .unwrap()
        .split("function ready")
        .next()
        .unwrap();
    assert!(settle.contains("for (const detach of stylesheetListeners.splice(0)) detach();"));
    assert!(settle.contains("window.removeEventListener(\"pagehide\", pagehide);"));
    for forbidden in [
        ".href =",
        ".media =",
        "createElement(\"link\")",
        ".disabled = false",
    ] {
        assert!(!loader.contains(forbidden), "{forbidden}");
    }
}

#[test]
fn dom_ready_gates_binding_evaluation_but_not_terminal_failure() {
    let loader = loader();
    assert_eq!(loader.matches("\"DOMContentLoaded\"").count(), 1);
    assert!(
        loader.contains("document.addEventListener(\"DOMContentLoaded\", resolve, { once: true })")
    );
    assert!(loader.contains(
        "Promise.all([\n  domReady.then(() => { if (!settled) return import(\"./pkg/zxs.js\"); }),\n  stylesheets,\n])"
    ));
    assert_eq!(loader.matches("module.default()").count(), 1);
    let fail = loader
        .split("function fail(stage, error)")
        .nth(1)
        .unwrap()
        .split("function renderFailure(stage)")
        .next()
        .unwrap();
    let settle = fail.find("if (!settle(\"failed\")) return;").unwrap();
    let cancel = fail
        .find("document.dispatchEvent(new Event(\"zxs-startup-failed\"))")
        .unwrap();
    let render = fail
        .find("domReady.then(() => renderFailure(stage))")
        .unwrap();
    assert!(settle < cancel && cancel < render);
    assert!(!fail.contains("document.body"));
    assert!(loader.contains("document.getElementById(\"root\").hidden = true;"));
    assert!(loader.contains("document.body.append(panel)"));
    assert!(loader.contains("window.location.reload();"));
    assert_eq!(loader.matches("setTimeout(").count(), 1);
    assert!(!loader.contains(".style"));
}
