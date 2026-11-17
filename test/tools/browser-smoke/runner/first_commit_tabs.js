// Delivery controls, not data fixtures. All server calls and response bodies
// are real. Only delivery of an already-successful response can be delayed/lost.
const nativeFetch = window.fetch.bind(window);
const trace = window.__zxsFirst = { posts: [], reads: [], pausePost: false, pauseRead: false };
window.fetch = async (...args) => {
  const probe = new Request(...args);
  const url = new URL(probe.url);
  const write = url.origin === location.origin && probe.method === "POST" &&
    /^\/api\/file-manager\/files\/[^/]+\/first-commit$/.test(url.pathname);
  const read = url.origin === location.origin && probe.method === "GET" &&
    /^\/api\/operations\/[^/]+\/outcome$/.test(url.pathname);
  const command = write ? await probe.clone().json() : null;
  if (write) trace.posts.push(command.operation_id);
  if (read) trace.reads.push(url.pathname.split("/")[3]);
  const response = await nativeFetch(probe);
  if (response.ok && write && trace.pausePost) {
    trace.confirmed = command.operation_id;
    await new Promise(resolve => { trace.releasePost = resolve; });
    throw new TypeError("first commit response lost after real server publication");
  }
  if (response.ok && read && trace.pauseRead) {
    trace.seen = url.pathname.split("/")[3];
    await new Promise(resolve => { trace.releaseRead = resolve; });
  }
  return response;
};
