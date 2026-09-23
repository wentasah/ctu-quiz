// Live updates for the student and presentation views.
//
// The server pushes a small JSON snapshot over server-sent events whenever
// somebody answers, the question is closed or the teacher asks the next one.
// Only the counter is updated in place; anything that changes the layout
// reloads the page, so that it is rendered by the server, exactly like on a
// fresh visit. That is also how the next question of a lecture reaches the
// students: their page has been open all along and reloads itself.
(function () {
  "use strict";

  var live = document.getElementById("live");
  if (!live || !window.EventSource) {
    return;
  }

  var wasOpen = live.dataset.open === "true";
  // Empty on the presentation view, and while a room waits for a question.
  var shownQuestion = live.dataset.question || "";
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

    // The open/closed state and the question itself decide the whole
    // layout, so let the server re-render it. The guard keeps a page from
    // reloading forever once it shows what the server sent.
    if (wasOpen !== state.open || shownQuestion !== (state.question || "")) {
      window.location.reload();
    }
  });

  // The question or the room is gone, for instance because the lecture is
  // over. Reloading shows what the server has to say about it.
  source.addEventListener("gone", function () {
    source.close();
    window.location.reload();
  });

  source.addEventListener("error", function () {
    setOffline(true);
  });
})();
