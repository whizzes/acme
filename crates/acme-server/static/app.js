// acme dashboard — the ~40 hand-written lines spec §13.4 budgets for:
// clipboard buttons, keyboard shortcuts, and prefers-reduced-motion.
(function () {
  "use strict";

  document.addEventListener("click", function (event) {
    var button = event.target.closest("[data-copy]");
    if (!button) return;
    var text = button.getAttribute("data-copy");
    navigator.clipboard.writeText(text).then(function () {
      var original = button.textContent;
      button.textContent = "Copied";
      setTimeout(function () {
        button.textContent = original;
      }, 1000);
    });
  });

  document.addEventListener("keydown", function (event) {
    if (event.target.closest("input, textarea, select")) return;
    if (event.key === "/") {
      var search = document.querySelector("input[name=q]");
      if (search) {
        event.preventDefault();
        search.focus();
      }
      return;
    }
    if (event.key === "g") {
      window.__acmeGPending = true;
      setTimeout(function () {
        window.__acmeGPending = false;
      }, 600);
      return;
    }
    if (window.__acmeGPending) {
      window.__acmeGPending = false;
      if (event.key === "p") window.location.href = "/payments";
      if (event.key === "s") window.location.href = "/shipments";
      if (event.key === "t") window.location.href = "/traffic";
    }
  });

  if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
    document.documentElement.classList.add("reduced-motion");
  }
})();
