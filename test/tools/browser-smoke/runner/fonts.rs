//! Real font transport faults: delay original bytes or return a font-only 404.
//! Every other resource and API response passes through the real server unchanged.
use super::*;
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

struct FontProxy {
    origin: String,
    gate: Arc<(Mutex<bool>, Condvar)>,
    requested: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl FontProxy {
    fn start(upstream: &str, missing: bool) -> Result<Self> {
        let upstream: u16 = upstream
            .rsplit(':')
            .next()
            .ok_or("missing server port")?
            .parse()?;
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let origin = format!("http://{}", listener.local_addr()?);
        listener.set_nonblocking(true)?;
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let requested = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (g, r, s) = (gate.clone(), requested.clone(), stop.clone());
        let worker = thread::spawn(move || {
            let mut connections = Vec::new();
            while !s.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut client, _)) => {
                        let (gate, requested) = (g.clone(), r.clone());
                        connections.push(thread::spawn(move || -> std::io::Result<()> {
                            client.set_read_timeout(Some(Duration::from_secs(5)))?;
                            client.set_write_timeout(Some(Duration::from_secs(5)))?;
                            let mut request = Vec::new();
                            while !request.ends_with(b"\r\n\r\n") {
                                if request.len() >= 64 * 1024 { return Ok(()); }
                                let mut byte = [0];
                                client.read_exact(&mut byte)?;
                                request.push(byte[0]);
                            }
                            let request = String::from_utf8_lossy(&request);
                            let path = request.split_whitespace().nth(1).unwrap_or("");
                            if path.starts_with("/fonts/") && path.ends_with(".woff2") {
                                requested.fetch_add(1, Ordering::SeqCst);
                                if missing {
                                    client.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")?;
                                    return Ok(());
                                }
                                let (lock, signal) = &*gate;
                                let _guard = signal.wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(90), |released| !*released).unwrap();
                            }
                            let mut server = TcpStream::connect(("127.0.0.1", upstream))?;
                            server.set_read_timeout(Some(Duration::from_secs(10)))?;
                            // Force EOF framing without modifying response headers/body.
                            for line in request.split("\r\n").filter(|line| !line.is_empty()) {
                                if !line.to_ascii_lowercase().starts_with("connection:") {
                                    write!(server, "{line}\r\n")?;
                                }
                            }
                            server.write_all(b"Connection: close\r\n\r\n")?;
                            std::io::copy(&mut server, &mut client)?;
                            Ok(())
                        }));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10))
                    }
                    Err(_) => break,
                }
            }
            for connection in connections {
                let _ = connection.join();
            }
        });
        Ok(Self {
            origin,
            gate,
            requested,
            stop,
            worker: Some(worker),
        })
    }
    fn release(&self) {
        *self.gate.0.lock().unwrap() = true;
        self.gate.1.notify_all();
    }
}
impl Drop for FontProxy {
    fn drop(&mut self) {
        self.release();
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub(super) fn run(browser: &Browser, origin: &str) -> Result<()> {
    browser.cdp("Network.setCacheDisabled", json!({"cacheDisabled":true}))?;
    with_cleanup(
        || {
            for mode in ["delayed", "missing", "disposed"] {
                let proxy = FontProxy::start(origin, mode == "missing")?;
                // WebDriver navigation waits for window load, which may include fonts.
                // CDP starts navigation without conflating it with app readiness.
                browser.cdp(
                    "Page.navigate",
                    json!({"url":format!("{}/?lang=en", proxy.origin)}),
                )?;
                let input = browser.css(
                    "form[data-auth-form='login'] input[autocomplete='username']:not([readonly])",
                )?;
                browser.keys(&input, "fallback-is-usable")?;
                wait_for("both original font HTTP requests", || {
                    if proxy.requested.load(Ordering::SeqCst) >= 2 {
                        Ok(())
                    } else {
                        Err("font requests have not reached proxy".into())
                    }
                })?;
                let state = browser.script("return {startup:document.documentElement.dataset.zxsStartup,fonts:document.documentElement.dataset.zxsFonts,value:document.querySelector('input[autocomplete=username]').value,error:!!document.getElementById('zxs-startup-error')}")?;
                if state["startup"] != "ready"
                    || state["value"] != "fallback-is-usable"
                    || state["error"] != false
                    || (mode != "missing" && state["fonts"] != "pending")
                {
                    return Err(format!("{mode} font blocked functional readiness: {state}").into());
                }
                if mode == "disposed" {
                    browser.script("window.dispatchEvent(new PageTransitionEvent('pagehide',{persisted:false})); return true")?;
                }
                proxy.release();
                wait_for("font transport and decoding settled", || {
                    if browser.script("return document.fonts.status === 'loaded'")? == true {
                        Ok(())
                    } else {
                        Err("fonts still loading".into())
                    }
                })?;
                if mode == "disposed" {
                    // Permit queued native FontFaceSet/Promise callbacks to run.
                    thread::sleep(Duration::from_millis(100));
                    if browser.script("return document.documentElement.dataset.zxsFonts === 'pending' && document.getElementById('root').childNodes.length === 0 && window.__zxsSmokeErrors.length === 0")? != true {
                    return Err("late font completion mutated disposed app".into());
                }
                } else {
                    let expected = if mode == "missing" { "failed" } else { "ready" };
                    wait_for("independent font result", || {
                        if browser.script(&format!("return document.documentElement.dataset.zxsFonts === '{expected}' && document.documentElement.dataset.zxsStartup === 'ready' && !document.getElementById('zxs-startup-error')"))? == true { Ok(()) }
                    else { Err("font result or functional readiness incorrect".into()) }
                    })?;
                    browser.keys(
                        &browser.css("form[data-auth-form='login'] input[autocomplete='username']:not([readonly])")?,
                        "-still-usable",
                    )?;
                    if mode == "delayed" {
                        wait_for("font-reflow contour geometry", || {
                            let paint = browser.command(
                                "POST",
                                "/execute/async",
                                Some(json!({
                                    "script": PAINT_OBSERVER, "args": []
                                })),
                            )?;
                            validate_paint(&paint).map_err(|reason| {
                                format!("{reason}; observed paint: {paint}").into()
                            })
                        })?;
                    }
                }
                println!(
                    "PASS real {mode} font transport: usable fallback, independent settlement and lifetime fencing"
                );
                browser.cdp("Page.navigate", json!({"url":"about:blank"}))?;
            }
            Ok(())
        },
        || {
            browser.cdp("Network.setCacheDisabled", json!({"cacheDisabled":false}))?;
            Ok(())
        },
    )
}

// Read the actual renderer output, not an assumed decoration child. Plain solid
// backgrounds use a compact nine-slice border-image; its source dimensions are
// intentionally independent of the host's size. Image decoding respects CSP's
// img-src blob allowance (fetching the blob would require connect-src blob).
const PAINT_OBSERVER: &str = r#"
const done=arguments[arguments.length-1];
(async () => {
  const node=document.querySelector('.authentication-card');
  if(!node) return {error:'missing real login card'};
  const css=getComputedStyle(node), box=node.getBoundingClientRect();
  const match=css.borderImageSource.match(/^url\(["']?(blob:[^"')]+)["']?\)$/);
  if(!match) return {error:'missing active border-image',source:css.borderImageSource,state:node.dataset.zxsContourState};
  const source=css.borderImageSource;
  const image=new Image();
  image.src=match[1];
  await image.decode();
  const canvas=document.createElement('canvas');
  canvas.width=image.naturalWidth; canvas.height=image.naturalHeight;
  const context=canvas.getContext('2d');
  context.drawImage(image,0,0);
  const raw=node.getAttribute('data-zxs-contour-extent') || css.getPropertyValue('--zxs-contour-extent');
  const requested=parseFloat(raw)*(raw.trim().endsWith('%') ? Math.min(box.width,box.height)/100 : 1);
  return {
    fonts:document.fonts.check('16px ZxsUI') && document.fonts.check('bold 16px ZxsUI'),
    stable:node.isConnected && getComputedStyle(node).borderImageSource===source,
    state:node.dataset.zxsContourState,
    width:box.width,height:box.height,extent:Math.min(requested,box.width/2,box.height/2),
    imageWidth:image.naturalWidth,imageHeight:image.naturalHeight,
    slice:css.borderImageSlice,borderWidth:parseFloat(css.borderImageWidth),outset:parseFloat(css.borderImageOutset),
    cornerAlpha:context.getImageData(0,0,1,1).data[3],
    curveCornerAlpha:context.getImageData(Math.ceil(parseFloat(css.borderImageOutset)),Math.ceil(parseFloat(css.borderImageOutset)),1,1).data[3],
    centerAlpha:context.getImageData(Math.floor(canvas.width/2),Math.floor(canvas.height/2),1,1).data[3]
  };
})().then(done,error=>done({error:String(error)}));
"#;

fn validate_paint(paint: &Value) -> std::result::Result<(), &'static str> {
    let number = |key: &str| paint[key].as_f64().filter(|value| value.is_finite());
    let (
        Some(width),
        Some(height),
        Some(extent),
        Some(image_width),
        Some(image_height),
        Some(border_width),
        Some(outset),
    ) = (
        number("width"),
        number("height"),
        number("extent"),
        number("imageWidth"),
        number("imageHeight"),
        number("borderWidth"),
        number("outset"),
    )
    else {
        return Err("missing finite paint geometry");
    };
    if paint["fonts"] != true
        || paint["stable"] != true
        || paint["state"] != "paint"
        || width <= 0.0
        || height <= 0.0
        || extent <= 0.0
        || outset <= 0.0
        || paint["cornerAlpha"] != 0
        || paint["curveCornerAlpha"] != 0
        || paint["centerAlpha"].as_u64().unwrap_or(0) == 0
    {
        return Err("font, live paint or decoded rounded-image check failed");
    }
    let slice = paint["slice"].as_str().ok_or("missing nine-slice CSS")?;
    if !slice.split_whitespace().any(|part| part == "fill") {
        return Err("missing painted center fill");
    }
    let first = slice
        .split_whitespace()
        .next()
        .ok_or("missing slice value")?;
    let close = |a: f64, b: f64| (a - b).abs() < 0.6; // SVG natural dimensions round to image pixels.
    if let Some(percent) = first.strip_suffix('%') {
        let percent: f64 = percent.parse().map_err(|_| "invalid percentage slice")?;
        // Compact tile: E-pixel corners + 2-pixel margin + one straight center pixel.
        if !close(border_width, extent + outset)
            || !close(image_width, 2.0 * border_width + 1.0)
            || !close(image_height, image_width)
            || !close(percent / 100.0 * image_width, border_width)
        {
            return Err("compact nine-slice corner size differs from host geometry");
        }
    } else {
        let slice: f64 = first.parse().map_err(|_| "invalid pixel slice")?;
        if !close(slice, outset)
            || !close(border_width, outset)
            || !close(image_width, width + 2.0 * outset)
            || !close(image_height, height + 2.0 * outset)
        {
            return Err("full-size border image differs from current host geometry");
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../unit/fonts_tests.rs"]
mod tests;
