// Live updates for the student and presentation views.
//
// The server pushes a small JSON snapshot over server-sent events whenever
// somebody answers or the question is closed. Only the counter is updated in
// place; closing the question reloads the page so that the bar graph is
// rendered by the server, exactly like on a fresh visit.
(function () {
  "use strict";

  var live = document.getElementById("live");
  if (!live || !window.EventSource) {
    return;
  }

  var wasOpen = live.dataset.open === "true";
  var source = new EventSource(live.dataset.events);

  function setOffline(offline) {
    var note = document.getElementById("offline");
    if (offline && !note) {
      note = document.createElement("div");
      note.id = "offline";
      note.textContent = "Připojení přerušeno, obnovuji…";
      document.body.appendChild(note);
    } else if (!offline && note) {
      note.remove();
    }
  }

  source.addEventListener("state", function (event) {
    setOffline(false);
    var state = JSON.parse(event.data);

    document.querySelectorAll("[data-count]").forEach(function (node) {
      node.textContent = state.answer_count;
    });

    // The open/closed state decides the whole layout, so let the server
    // re-render it. The guard keeps a closed page from reloading forever.
    if (wasOpen !== state.open) {
      window.location.reload();
    }
  });

  source.addEventListener("error", function () {
    setOffline(true);
  });
})();
