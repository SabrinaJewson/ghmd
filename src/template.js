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
