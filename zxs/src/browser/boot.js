import("./zxs_browser.js")
  .then(({ default: init }) => init())
  .catch(() => {
    document.getElementById("zxs-folder-load-error")?.removeAttribute("hidden");
  });
