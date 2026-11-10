// InkVoice landing page: the follow-along demo, platform-aware download button,
// live release info, and the copy button. No dependencies.
(() => {
  const REPO = "sabeel111/InkVoice";
  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  // ------------------------------------------------------------- reading demo
  const demo = document.querySelector(".demo");
  const textEl = document.getElementById("demo-text");
  const audio = document.getElementById("demo-audio");
  const button = document.getElementById("listen");
  const label = button.querySelector(".listen-label");

  // wrap each word in a span, keep the spaces between them
  const words = textEl.textContent.trim().split(/\s+/);
  textEl.textContent = "";
  const spans = words.map((w, i) => {
    const s = document.createElement("span");
    s.className = "w";
    s.textContent = w;
    textEl.append(s);
    if (i < words.length - 1) textEl.append(" ");
    return s;
  });
  const marker = document.createElement("span");
  marker.className = "marker";
  marker.setAttribute("aria-hidden", "true");
  textEl.prepend(marker);

  let timings = null; // [[word, start, end], ...] made by InkVoice itself
  fetch("media/hero.json").then(r => r.json()).then(j => { timings = j.words; }).catch(() => {});

  let current = -1;
  let lastTop = null;
  function mark(i) {
    if (i === current) return;
    current = i;
    spans.forEach((s, k) => {
      s.classList.toggle("said", k <= i);
      s.classList.toggle("now", k === i);
    });
    if (i < 0) { marker.classList.remove("on"); lastTop = null; return; }
    const s = spans[i];
    const pad = 4;
    const top = s.offsetTop;
    // glide along a line, but jump (no slide) when the reading moves to the next line
    marker.classList.toggle("jump", lastTop !== null && top !== lastTop);
    lastTop = top;
    marker.style.width = `${s.offsetWidth + pad * 2}px`;
    marker.style.height = `${s.offsetHeight}px`;
    marker.style.transform = `translate(${s.offsetLeft - pad}px, ${top}px)`;
    marker.classList.add("on");
  }

  function wordAt(t) {
    if (!timings) return Math.min(spans.length - 1, Math.floor((t / 7.2) * spans.length));
    let idx = -1;
    for (let k = 0; k < timings.length; k++) if (timings[k][1] <= t) idx = k;
    return Math.min(idx, spans.length - 1);
  }

  let raf = 0;
  function follow() {
    mark(wordAt(audio.currentTime));
    raf = requestAnimationFrame(follow);
  }

  function setPlaying(on) {
    button.setAttribute("aria-pressed", String(on));
    label.textContent = on ? "Pause" : audio.currentTime > 0 && !audio.ended ? "Resume" : "Listen";
    demo.classList.toggle("playing", on || (audio.currentTime > 0 && !audio.ended));
  }

  button.addEventListener("click", () => {
    stopIntro();
    if (audio.paused) {
      if (audio.ended) audio.currentTime = 0;
      audio.play().catch(() => {});
    } else {
      audio.pause();
    }
  });
  audio.addEventListener("play", () => { setPlaying(true); cancelAnimationFrame(raf); follow(); });
  audio.addEventListener("pause", () => { setPlaying(false); cancelAnimationFrame(raf); });
  audio.addEventListener("ended", () => {
    cancelAnimationFrame(raf);
    setPlaying(false);
    demo.classList.remove("playing");
    setTimeout(() => { if (audio.paused) mark(-1); }, 1200);
  });
  window.addEventListener("resize", () => { const c = current; current = -1; lastTop = null; mark(c); });

  // one quiet introduction on load: the highlight reads the sentence once, silently
  let introTimer = 0;
  function stopIntro() { clearTimeout(introTimer); introTimer = 0; }
  if (!reduced) {
    const start = performance.now() + 900;
    const step = () => {
      const t = (performance.now() - start) / 1000;
      if (t < 0) { introTimer = setTimeout(step, 60); return; }
      if (t > 7.4) { introTimer = setTimeout(() => mark(-1), 900); return; }
      mark(wordAt(t));
      introTimer = setTimeout(step, 40);
    };
    introTimer = setTimeout(step, 60);
  }

  // ------------------------------------------------------------- platform + release info
  const ua = (navigator.userAgentData && navigator.userAgentData.platform) || navigator.userAgent || "";
  const os = /win/i.test(ua) ? "windows" : /android|iphone|ipad/i.test(ua) ? "mobile" : /mac/i.test(ua) ? "mac" : /linux|x11|cros/i.test(ua) ? "linux" : "other";
  const cta = document.getElementById("cta");
  const ctaMeta = document.getElementById("cta-meta");
  const setup = `https://github.com/${REPO}/releases/latest/download/InkVoice-Setup-x86_64.exe`;

  if (os === "windows") {
    cta.textContent = "Download for Windows";
    cta.href = setup;
    document.getElementById("dl-windows").classList.add("mine");
  } else if (os === "linux") {
    cta.textContent = "Get it for Linux";
    cta.href = "#dl-linux";
    document.getElementById("dl-linux").classList.add("mine");
  }

  const mb = n => `${(n / 1e6).toFixed(1)} MB`;
  fetch(`https://api.github.com/repos/${REPO}/releases/latest`)
    .then(r => (r.ok ? r.json() : Promise.reject()))
    .then(rel => {
      const v = rel.tag_name.replace(/^v/, "");
      const find = name => rel.assets.find(a => a.name === name);
      if (os === "windows" && find("InkVoice-Setup-x86_64.exe")) {
        ctaMeta.textContent = `Version ${v} for Windows 10 and 11, ${mb(find("InkVoice-Setup-x86_64.exe").size)}`;
      } else if (os === "linux" && find("inkvoice-linux-x86_64.tar.gz")) {
        ctaMeta.textContent = `Version ${v} for Linux, ${mb(find("inkvoice-linux-x86_64.tar.gz").size)}`;
      } else {
        ctaMeta.textContent = `Version ${v}, free for Windows and Linux`;
      }
    })
    .catch(() => {});

  // ------------------------------------------------------------- copy command
  document.querySelectorAll(".copy").forEach(b => {
    b.addEventListener("click", async () => {
      const text = document.getElementById(b.dataset.copy).textContent;
      try {
        await navigator.clipboard.writeText(text);
        b.textContent = "Copied";
        b.classList.add("done");
        setTimeout(() => { b.textContent = "Copy"; b.classList.remove("done"); }, 1800);
      } catch {
        const r = document.createRange();
        r.selectNodeContents(document.getElementById(b.dataset.copy));
        getSelection().removeAllRanges();
        getSelection().addRange(r);
      }
    });
  });
})();
