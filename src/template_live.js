const events = new EventSource("/");
events.addEventListener("update", e => {
	document.getElementsByTagName("main")[0].innerHTML = e.data;
});
events.addEventListener("rate_limited", e => {
	console.log(e.data);
});
events.addEventListener("render_error", e => {
	console.log(e.data);
});

let initial_connect = true;
events.addEventListener("open", () => {
	if (!initial_connect) {
		location.reload();
	}
	initial_connect = false;
});
