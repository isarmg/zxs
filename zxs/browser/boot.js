import init from "./zxs_browser.js";

init().catch(() => {
  document.getElementById("zxs-folder-load-error")?.removeAttribute("hidden");
});
