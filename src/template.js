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

function correct_hash_scroll() {
	if (location.hash !== "" && document.querySelector(":target") === null) {
		const element = document.getElementById(`user-content-${location.hash.slice(1)}`);
		if (element !== null) {
			element.scrollIntoView();
		}
	}
}
addEventListener("hashchange", correct_hash_scroll);
addEventListener("load", correct_hash_scroll);
