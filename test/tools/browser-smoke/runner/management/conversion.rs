//! Real disposable source + empty-target conversion through the production UI.
use super::*;

const OBSERVE: &str = r#"
if(window.__zxsConversionProbe)throw new Error('Conversion observer already installed');
const original=window.fetch;
const state=window.__zxsConversionProbe={posts:[],preview:null,commit:null,restore:()=>{window.fetch=original;delete window.__zxsConversionProbe;}};
window.fetch=async function(...args){
  const input=args[0],url=new URL(typeof input==='string'?input:input.url,location.href);
  const method=(args[1]?.method||input.method||'GET').toUpperCase();
  const observe=url.origin===location.origin&&url.pathname.startsWith('/api/admin/storage/conversion/');
  if(observe&&method==='POST')state.posts.push(url.pathname);
  const response=await Reflect.apply(original,this,args);
  if(observe&&method==='POST'){
    const record={status:response.status,body:await response.clone().json()};
    if(url.pathname.endsWith('/preview'))state.preview=record;
    if(url.pathname.endsWith('/commit'))state.commit=record;
  }
  return response;
};return true;
"#;

pub(super) fn run(browser: &Browser, fixture: &SetupFixture) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    let parent = fixture.root.join("storage-parent");
    let source = parent.join("ordinary-source");
    let target = parent.join("converted-primary");
    for path in [&source, &target] {
        fs::DirBuilder::new().mode(0o700).create(path)?;
    }
    let contents = b"original ordinary directory contents\n";
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(source.join("ordinary.txt"))?
        .write_all(contents)?;
    let content_bytes = contents.len().to_string();
    let configuration = fs::read(fixture.pool.join("config.json"))?;
    let source_json = json_codec::to_string(source.to_str().ok_or("non-UTF8 source")?)?;
    let target_json = json_codec::to_string(target.to_str().ok_or("non-UTF8 target")?)?;
    browser.script(OBSERVE)?;
    with_cleanup(
        || {
            browser.click(&browser.button("Administration", "系统管理")?)?;
            browser.click(&browser.css("[data-storage-discover]:not([disabled])")?)?;
            browser.click(&browser.css(&format!(
                "[data-storage-candidate={source_json}]:not([disabled])"
            ))?)?;
            browser.css("[data-conversion-preview]")?;
            browser.click(&browser.css(&format!(
                "[data-conversion-target={target_json}]:not([disabled])"
            ))?)?;
            browser.click(&browser.css("[data-conversion-preview]:not([disabled])")?)?;
            browser.css("[data-conversion-review]")?;
            let preview = browser.script("return window.__zxsConversionProbe.preview")?;
            let body = &preview["body"];
            if preview["status"] != 200
                || body["source"]["path"] != source.to_str().ok_or("non-UTF8 source")?
                || body["source_preview"]["files"] != 1
                || body["source_preview"]["content_bytes"].as_str() != Some(content_bytes.as_str())
                || body["target"]["request"]["additions"]
                    .as_array()
                    .map(Vec::len)
                    != Some(1)
                || body["target"]["request"]["additions"][0]["level"] != 1
                || !body["target"]["request"]["additions"][0]["upstream_slot_id"].is_null()
                || body["target"]["request"]["additions"][0]["selection"]["root_path"]
                    != target.to_str().ok_or("non-UTF8 target")?
            {
                return Err(format!("ordinary conversion review did not bind the real source and new primary: {preview}").into());
            }
            if fs::read(fixture.pool.join("config.json"))? != configuration
                || fs::read_dir(&target)?.next().is_some()
                || fs::read(source.join("ordinary.txt"))? != contents
            {
                return Err("conversion preview changed configuration, target or source".into());
            }
            if browser.script(
                "return document.querySelector('[data-conversion-commit]')?.disabled===true",
            )? != true
            {
                return Err("conversion committed without contents-only confirmation".into());
            }
            browser.click(&browser.css("[data-conversion-confirm]")?)?;
            browser.css("[data-conversion-commit]:not([disabled])")?;
            // Two actual UI click stimuli share one intent; the application fence
            // must suppress the second without changing the returned server result.
            browser.script("const button=document.querySelector('[data-conversion-commit]');button.click();button.click();return true")?;
            wait_for("real conversion publication and UI completion", || {
                let observation=browser.script("const p=window.__zxsConversionProbe;return {receipt:p.commit,posts:p.posts,ui:document.querySelector('[data-conversion-status]')?.textContent||''}")?;
                let receipt = &observation["receipt"];
                if receipt["status"] == 200
                    && receipt["body"]["phase"] == "completed"
                    && receipt["body"]["preview"] == *body
                    && receipt["body"]["registration"]["state"] == "completed"
                    && receipt["body"]["result"]["kind"] == "workspace.commit"
                    && receipt["body"]["result"]["operation_id"] == body["import_operation_id"]
                    && observation["posts"]
                        == json!([
                            "/api/admin/storage/conversion/preview",
                            "/api/admin/storage/conversion/commit"
                        ])
                    && observation["ui"].as_str().is_some_and(|text| {
                        text.contains("Conversion completed") || text.contains("转换完成")
                    })
                {
                    Ok(())
                } else {
                    Err(format!("conversion not proven complete: {observation}").into())
                }
            })?;
            browser.css(".zxs-disk-tree-node[data-slot='N01']")?;
            wait_for("converted target leaves unadmitted tier", || {
                if browser.script(&format!("return ![...document.querySelectorAll('[data-storage-candidate]')].some(node=>node.dataset.storageCandidate==={target_json})"))?==true {Ok(())}
            else {Err("registered conversion target remains in pending tier".into())}
            })?;
            if fs::read(source.join("ordinary.txt"))? != contents
                || fs::read_dir(&source)?.count() != 1
            {
                return Err("conversion modified or erased the original source".into());
            }
            browser.click(&browser.button("Files", "文件")?)?;
            browser.click(&browser.css(".zxs-disk-entry[data-slot='N01']:not([disabled])")?)?;
            browser.element(
                "xpath",
                "//button[contains(@class,'zxs-file-name')][normalize-space(.)='ordinary.txt']",
            )?;
            println!(
                "PASS real ordinary-source preview without writes, explicit new-primary conversion, single commit, permanent import receipt, pending-tier removal and original source preservation"
            );
            Ok(())
        },
        || {
            browser.script(
                "if(window.__zxsConversionProbe)window.__zxsConversionProbe.restore();return true",
            )?;
            Ok(())
        },
    )
}
