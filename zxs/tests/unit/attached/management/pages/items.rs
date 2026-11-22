#[test]
fn contours_are_inert_local_decoration_on_login_and_overview() {
    let login = sign_in(true);
    let details = browser_overview(&Overview { login: "A<&".into(), shares: Vec::new(), next: None }, "example").unwrap();
    for (page, tiles) in [(&login, 4), (&details, 12)] {
        assert_eq!(page.matches("id=\"zxs-g4-corner\"").count(), 1);
        assert_eq!(page.matches("href=\"#zxs-g4-corner\"").count(), tiles);
        assert!(page.contains("class=\"zxs-g4-paint\" aria-hidden=\"true\""));
        assert!(!page.contains(" style="));
    }
    assert!(!login.contains("<script"));
    assert_eq!(details.matches(r#"<script type="module" src="/ui/v1/boot.js"></script>"#).count(), 1);
    assert!(login.contains("action=\"/login\"") && login.contains("autocomplete=\"current-password\""));
    assert!(login.contains("HTTP 未加密") && login.contains("role=\"alert\""));
    assert!(sign_in(false).contains("HTTP 未加密"));
    assert!(details.contains("<bdi>A&lt;&amp;</bdi>") && details.contains("action=\"/logout\""));
}

#[test]
fn overview_controls_retain_each_original_share_and_previous_operation() {
    use crate::{VolumeId, identity::ShareId, system::reader::account::overview::Share};
    let id = ShareId([0x13; 16]);
    let previous = OperationId([0x24; 16]);
    let volume = VolumeId([0x35; 16]);
    let view = Overview {
        login: "demo".into(),
        shares: vec![Share { share: id, previous, volume, label: "甲<&".into(), mode: Mode::ReadOnly }],
        next: Some(id),
    };
    let html = browser_overview(&view, "A&").unwrap();
    assert!(html.contains(&format!(r#"id="zxs-share-{id}">甲&lt;&amp;</bdi>"#)));
    assert!(html.contains(&format!(r#"<code class="zxs-volume-value">{volume}</code>"#)));
    assert!(html.contains(&format!(r#"href="/?after={id}#zxs-shares""#)));
    assert!(html.contains(&format!(r#"name="share" value="{id}""#)));
    assert!(html.contains(&format!(r#"name="previous" value="{previous}""#)));
    assert!(html.contains(r#"name="op" value=""#) && html.matches(r#"name="csrf" value="A&amp;""#).count() == 2);
    assert!(html.contains(r#"name="mode""#) && html.contains(r#"<option value="ro" selected>"#));
}

#[test]
fn an_unsettled_parent_never_becomes_an_old_path_or_a_submit_link(){
    use crate::VolumeId;
    use manual_work::{Item,PickerItem};
    let blocked=FileId([0x20;16]);let other=FileId([0x22;16]);
    let page=manual_index(&[PickerItem::ParentPending(blocked),PickerItem::Named(Item{
        file:other,name:"Other.txt".into(),volume:VolumeId([0x42;16]),
    })],None);
    let body=std::str::from_utf8(&page.body).unwrap();
    assert!(body.contains(&format!("<code>{blocked}</code>"))&&body.contains("所属目录改名尚未确认"));
    assert!(!body.contains(&format!("?file={blocked}")));
    assert!(body.contains(&format!("/versions?file={other}"))&&body.contains("Other.txt"));
}

#[test]
fn pending_parent_history_displays_only_original_identity_versions_and_read_navigation(){
    use crate::{NodeId,VersionId,system::reader::account::history::Version};
    let file=FileId([0x41;16]);let version=VersionId([0x42;16]);let node=NodeId([0x43;16]);
    let history=Timeline{file,place:TimelinePlace::ParentPending,next:Some(1),
        rows:vec![Version{order:2,node,version,parent:Some(NodeId([0x44;16])),revision:3,length:7}]};
    let page=version_history(&history);
    let body=std::str::from_utf8(&page.body).unwrap();
    assert!(body.contains(&format!("<code>{file}</code>"))&&body.contains("所属目录改名尚未确认"));
    assert!(body.contains(&format!("<code>{version}</code>"))&&body.contains(&format!("<code>{node}</code>")));
    assert!(body.contains(&format!("/versions/history?file={file}&amp;before=1")));
    for unwarranted in ["<bdi>","当前卷内路径","数据卷","回收卷","/versions?file=","<form","/download"]{
        assert!(!body.contains(unwarranted));
    }
}
