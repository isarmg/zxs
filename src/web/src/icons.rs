use web_interface::prelude::*;
#[component]
pub fn FileIcon(name: &'static str) -> impl IntoView {
    let path = match name {
        "folder" => "M3 20V5h6l3 3h8v3M3 20h16l3-9H7l-4 9",
        "file" => "M6 3h8l4 4v14H6V3m8 0v5h4M9 12h6m-6 4h6",
        "home" => "m3 10 9-8 9 8M5 8v13h14V8M9 21v-8h6v8",
        "move" => "M4 4v7a5 5 0 0 0 5 5h11m-5-5 5 5-5 5",
        "delete" => "M4 6h16M9 6V3h6v3M6 6l1 15h10l1-15M10 10v7m4-7v7",
        "rename" => "m4 17-1 4 4-1L21 6l-4-4L4 17m10-12 4 4",
        "content" => "M6 3h8l4 4v14H6V3m8 0v5h4M9 12h6m-6 4h4",
        "merge" => "M6 3v5c0 4 12 4 12 8v5M18 3v18m-4-4 4 4 4-4",
        "path-change" => "M3 6h7l3 4h8M3 18h7l3-4h8m-3-7 3 3-3 3m-12 2-3 3 3 3",
        "logout" => "M10 3H3v18h7m5-15 6 6-6 6m-8-6h14",
        "refresh" => "M19.518 9.264a8 8 0 1 1-4.782-4.782",
        "account" => "M20 21a8 8 0 0 0-16 0M12 3a4 4 0 1 0 0 8 4 4 0 0 0 0-8",
        "create" => "M12 3v18M3 12h18",
        "cpu" => {
            "M7 7h10v10H7V7m-4 2h4m-4 6h4m10-6h4m-4 6h4M9 3v4m6-4v4M9 17v4m6-4v4M10 10h4v4h-4v-4"
        }
        "memory" => "M3 6h18v12H3V6m3 12v3m4-3v3m4-3v3m4-3v3M7 9v5m5-5v5m5-5v5",
        "disk" => "M5 3h14l3 14v4H2v-4L5 3M2 17h20M6 19h.01M10 19h.01",
        "power" => "m13 2-9 12h7l-1 8 10-13h-7l0-7",
        _ => "M5 3h9l5 5v13H5V3m9 0v6h5",
    };
    let icon_class = match name {
        "folder" => "zxs-file-icon zxs-file-icon--folder",
        "file" => "zxs-file-icon zxs-file-icon--file",
        "refresh" => "zxs-file-icon zxs-refresh-icon",
        _ => "zxs-file-icon",
    };
    view! {
        <svg
            class=icon_class
            aria-hidden="true"
            width="1em"
            height="1em"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="1.7"
            stroke-linecap=if name == "refresh" { "round" } else { "butt" }
        >
            <path d=path/>
        </svg>
    }
}
