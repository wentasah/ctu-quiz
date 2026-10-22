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
  var wasRevealed = live.dataset.revealed === "true";
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

    // The open/closed state, the revealed answers and the question itself
    // decide the whole layout, so let the server re-render it. The guard
    // keeps a page from reloading forever once it shows what the server
    // sent.
    if (
      wasOpen !== state.open ||
      wasRevealed !== Boolean(state.revealed) ||
      shownQuestion !== (state.question || "")
    ) {
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

// Counts down the time until a question asked with `--close-after` closes by
// itself, on its button in the presentation view. The server says how much
// time was left when it rendered the page, so the browser's clock does not
// matter; once the server closes the question, the event stream reloads the
// page.
(function () {
  "use strict";

  var button = document.querySelector("button.countdown");
  if (!button) {
    return;
  }

  var remaining = Number(button.dataset.remainingMs);
  var total = Number(button.dataset.totalMs);
  var start = performance.now();

  function tick(now) {
    var left = Math.max(0, remaining - (now - start)) / total;
    button.style.setProperty("--left", left);
    if (left > 0) {
      window.requestAnimationFrame(tick);
    }
  }

  window.requestAnimationFrame(tick);
})();

// Shrinks the question and the QR code box of the presentation view when they
// would not fit into the window, so that nothing has to be scrolled on the
// projector. The stylesheet sizes both boxes from their `--fit`; this finds
// the largest value, at most 1, with which a box still fits.
(function () {
  "use strict";

  if (!document.body.classList.contains("display")) {
    return;
  }

  var boxes = document.querySelectorAll("body.display .question, body.display .join");
  // With one column the boxes are below each other and the page scrolls
  // anyway.
  var oneColumn = window.matchMedia("(max-width: 60rem)");
  var smallest = 0.3;

  function fits(box, fit) {
    box.style.setProperty("--fit", fit);
    var bottom = box.getBoundingClientRect().bottom + window.scrollY;
    var padding = parseFloat(getComputedStyle(document.body).paddingBottom) || 0;
    return bottom <= window.innerHeight - padding;
  }

  function fitAll() {
    boxes.forEach(function (box) {
      if (oneColumn.matches || fits(box, 1)) {
        box.style.setProperty("--fit", 1);
        return;
      }
      var low = smallest;
      var high = 1;
      for (var i = 0; i < 10; i++) {
        var middle = (low + high) / 2;
        if (fits(box, middle)) {
          low = middle;
        } else {
          high = middle;
        }
      }
      box.style.setProperty("--fit", low);
    });
  }

  fitAll();
  window.addEventListener("resize", fitAll);
  if (document.fonts) {
    document.fonts.ready.then(fitAll);
  }
})();
