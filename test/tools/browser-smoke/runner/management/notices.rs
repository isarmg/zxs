//! Real account-only setup must not look like a storage fault on every page.
use super::{Browser, Result, json, wait_for};

pub(super) fn unconfigured(browser: &Browser) -> Result<()> {
    browser.click(&browser.button("Administration", "系统管理")?)?;
    wait_for(
        "unconfigured storage has an explanatory management status",
        || {
            if browser.script(r#"
            const recovery=document.querySelector('.zxs-management-recovery');
            if(!recovery)return false;
            const buttons=[...recovery.querySelectorAll('button')].map(e=>e.textContent.trim());
            return /Storage has not been added|尚未添加存储/.test(recovery.textContent)
                && buttons.some(text=>['Check management status','检查管理状态'].includes(text))
                && !buttons.some(text=>['Retry opening data storage','重试打开数据存储'].includes(text));
        "#)? == true { Ok(()) }
        else { Err("unconfigured management status is missing or exposes an idle retry".into()) }
        },
    )?;
    for (english, chinese) in [
        ("Files", "文件"),
        ("Details", "详情"),
        ("Status", "状态"),
        ("History", "历史记录"),
        ("Files", "文件"),
    ] {
        browser.click(&browser.button(english, chinese)?)?;
        wait_for(
            "unconfigured navigation has no global recovery buttons",
            || {
                if browser.script(&format!(r#"
                const labels={},active=[...document.querySelectorAll('[aria-current=page]')];
                const names=['Check management status','检查管理状态','Retry opening data storage','重试打开数据存储'];
                const status=document.querySelector('section[aria-label="Storage and authentication status"], section[aria-label="存储与认证状态"]');
                return active.some(e=>labels.includes(e.textContent.trim())) && !status
                    && ![...document.querySelectorAll('button')].some(e=>names.includes(e.textContent.trim()));
            "#, json!([english, chinese])))? == true { Ok(()) }
            else { Err(format!("unconfigured {english} retained global recovery controls").into()) }
            },
        )?;
    }
    println!(
        "PASS real unconfigured storage: management keeps status context; repeated page navigation has no global recovery controls"
    );
    Ok(())
}
