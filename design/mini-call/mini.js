// Sidevoice · mini llamada — maquetas interactivas. Todo es simulado: ningún dato sale de la página.

const MARK = `<svg class="mark" viewBox="0 0 24 24"><rect class="b1" x="1.5" y="9" width="3" height="6" rx="1.5"/><rect class="b2" x="6" y="6" width="3" height="12" rx="1.5"/><rect class="b3" x="10.5" y="1.5" width="3" height="21" rx="1.5"/><rect class="b4" x="15" y="6" width="3" height="12" rx="1.5"/><rect class="b5" x="19.5" y="9" width="3" height="6" rx="1.5"/></svg>`;
// Tono en blanco (barra de menús, iconos monocromos): la cuarta barra al 45 %.
const MARK_WHITE = `<svg viewBox="0 0 24 24"><g fill="#fff"><rect x="1.5" y="9" width="3" height="6" rx="1.5"/><rect x="6" y="6" width="3" height="12" rx="1.5"/><rect x="10.5" y="1.5" width="3" height="21" rx="1.5"/><rect x="15" y="6" width="3" height="12" rx="1.5" fill-opacity=".45"/><rect x="19.5" y="9" width="3" height="6" rx="1.5"/></g></svg>`;
const SLASH = `<svg class="slash" viewBox="0 0 24 24"><line x1="3" y1="3" x2="21" y2="21" stroke="#ff8a80" stroke-width="2.2" stroke-linecap="round"/></svg>`;
const I = {
  mic: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><rect x="9" y="3" width="6" height="11" rx="3"/><path d="M5 11a7 7 0 0 0 14 0M12 18v3"/></svg>`,
  micOff: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><rect x="9" y="3" width="6" height="11" rx="3"/><path d="M5 11a7 7 0 0 0 14 0M12 18v3M3 3l18 18"/></svg>`,
  hang: `<svg viewBox="0 0 24 24" fill="currentColor"><path d="M12 9c-3.3 0-6.3.9-8.7 2.5-.6.4-.9 1.2-.6 1.9l1 2.2c.3.7 1.1 1 1.8.8l3-1c.6-.2 1-.8.9-1.4l-.2-1.6c1.8-.5 3.8-.5 5.6 0l-.2 1.6c-.1.6.3 1.2.9 1.4l3 1c.7.2 1.5-.1 1.8-.8l1-2.2c.3-.7 0-1.5-.6-1.9C18.3 9.9 15.3 9 12 9z"/></svg>`,
  skip: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round"><path d="M5 5l10 7-10 7z"/><path d="M19 5v14" stroke-linecap="round"/></svg>`,
  chev: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M6 9l6 6 6-6"/></svg>`,
  open: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M14 4h6v6M20 4l-8 8M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5"/></svg>`,
};

const STATES = {
  listening: { label: "Te escucho", short: "Escuchando" },
  transcribing: { label: "Transcribiendo…", short: "Transcribiendo" },
  thinking: { label: "{a} está trabajando…", short: "Trabajando" },
  speaking: { label: "{a} habla", short: "Habla {a}" },
  muted: { label: "Micrófono silenciado", short: "Silenciado" },
  reconnecting: { label: "Reconectando…", short: "Reconectando" },
  idle: { label: "Sin llamada", short: "Sin llamada" },
};
const STATE_ORDER = ["listening", "transcribing", "thinking", "speaking", "muted", "reconnecting", "idle"];

const CONVS = [
  { t: "sidevoice-desktop", a: "Claude", m: "portátil", work: false, unread: 0 },
  { t: "brand/web", a: "Claude", m: "portátil", work: true, unread: 2 },
  { t: "homelab", a: "Codex", m: "nas", work: false, unread: 0 },
  { t: "landing", a: "Cursor", m: "portátil", work: false, unread: 0, off: true },
];

const OPTIONS = {
  A: {
    name: "A · Píldora flotante",
    blurb: "Una ventanita propia, siempre encima y sin robar el foco, en una esquina. Se despliega y se esconde al borde.",
    rec: true,
  },
  B: {
    name: "B · Isla (notch)",
    blurb: "Una píldora negra pegada al notch del Mac que crece cuando pasa algo. Fuera del Mac, arriba en el centro.",
  },
  C: {
    name: "C · Solo la barra",
    blurb: "Nada flota: el icono de la barra muestra estado y tiempo, y al pulsarlo abre un panel con los controles.",
  },
  D: {
    name: "D · Controles del sistema",
    blurb: "Sin ventana propia: el reproductor del sistema (Centro de control, panel multimedia) hace de mando.",
  },
};

const S = {
  option: "A",
  platform: "mac", // mac | macnonotch | win | x11 | wayland
  state: "listening",
  prevState: "listening",
  mode: "compact", // compact | expanded | hidden
  conv: 0,
  start: Date.now() - 12 * 60 * 1000 - 34 * 1000,
  corner: "tr",
  armedHang: 0,
  thumb: true,
};

const $ = (s, r = document) => r.querySelector(s);
const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
const conv = () => CONVS[S.conv];
const stateText = (st, short) => (short ? STATES[st].short : STATES[st].label).replace("{a}", conv().a);
const clock = () => {
  const s = Math.floor((Date.now() - S.start) / 1000);
  const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60), x = s % 60;
  return (h ? h + ":" + String(m).padStart(2, "0") : m) + ":" + String(x).padStart(2, "0");
};
const isMac = () => S.platform === "mac" || S.platform === "macnonotch";
const mod = () => (isMac() ? "⌃⌥" : "Ctrl+Alt+");

// ---------- piezas ----------
function markBox(st, cls = "mk") {
  return `<div class="${cls} st-${st}">${MARK}${st === "muted" ? SLASH : ""}</div>`;
}
function micBtn(st) {
  const off = st === "muted";
  const dis = st === "reconnecting" || st === "idle";
  return `<button class="iconbtn ${off ? "muted" : ""}" data-act="mute" title="${off ? "Activar micrófono" : "Silenciar"} (${mod()}M)" ${dis ? "disabled" : ""}>${off ? I.micOff : I.mic}</button>`;
}
function hangBtn(st) {
  const armed = S.armedHang > Date.now();
  return `<button class="iconbtn hang" data-act="hang" title="Colgar (${mod()}H dos veces)" ${st === "idle" ? "disabled" : ""} style="${armed ? "outline:2px solid #fff" : ""}">${I.hang}</button>`;
}
function convList() {
  return `<div class="convlist">${CONVS.map((c, i) => `
    <button class="conv ${i === S.conv ? "sel" : ""}" data-conv="${i}">
      <span class="d ${c.off ? "off" : c.work ? "work" : ""}"></span>
      <span class="n">${esc(c.t)}<small>${esc(c.a)} · ${esc(c.m)}${c.off ? " · sin conexión" : c.work ? " · trabajando" : ""}</small></span>
      ${c.unread ? `<span class="badge">${c.unread}</span>` : ""}
      <kbd>${i + 1}</kbd>
    </button>`).join("")}</div>`;
}
function keysLine() {
  return `<div class="keys"><span><kbd>${mod()}M</kbd> silenciar</span><span><kbd>${mod()}H</kbd>×2 colgar</span><span><kbd>${mod()}Espacio</kbd> mando</span><span><kbd>1–9</kbd> conversación</span><span><kbd>S</kbd> saltar</span><span><kbd>Esc</kbd> cerrar</span></div>`;
}
function detailBlock(st, closeLabel = "Ocultar") {
  return `
    ${convList()}
    <div class="actions">
      <button class="chip" data-act="skip" ${st === "speaking" ? "" : "disabled"}>${"Saltar respuesta"}</button>
      <button class="chip" data-act="openapp">Abrir Sidevoice</button>
      <button class="chip" data-act="hide">${closeLabel}</button>
    </div>
    ${keysLine()}`;
}
const unreadElsewhere = () => CONVS.reduce((n, c, i) => n + (i === S.conv ? 0 : c.unread), 0);

// ---------- A · píldora ----------
function pillA(st, pos, { mode = S.mode, gallery = false } = {}) {
  if (st === "idle") return gallery ? `<div class="statusline" style="padding:10px">Sin llamada: la píldora no está. Solo el icono de la barra.</div>` : "";
  const unread = unreadElsewhere();
  if (mode === "hidden") {
    const style = gallery ? "position:relative;right:auto;top:auto" : `right:0;top:${pos.top}px`;
    return `<div class="edgetab hud" style="${style}" data-act="expand" title="Mostrar (${mod()}V)">${markBox(st)}${st === "muted" ? `<span class="mdot"></span>` : ""}</div>`;
  }
  const style = gallery ? "position:relative" : posStyle(pos);
  const pill = `
    <div class="pill hud st-${st}" style="${style}" data-act="${mode === "expanded" ? "" : "expand"}">
      <span class="drag" title="Arrastrar a otra esquina"></span>
      ${markBox(st)}
      <div class="txt"><div class="title">${esc(conv().t)} ${unread ? `<span class="badge" title="Sin leer en otras conversaciones">${unread}</span>` : ""}</div><div class="statusline">${esc(stateText(st))}</div></div>
      <span class="timer">${clock()}</span>
      ${micBtn(st)}${hangBtn(st)}
    </div>`;
  if (mode !== "expanded") return pill;
  const cardStyle = gallery ? "position:relative;margin-top:6px" : posStyle({ ...pos, top: pos.top + 52 });
  return pill + `<div class="pillcard hud st-${st}" style="${cardStyle}">${detailBlock(st)}</div>`;
}
function posStyle(p) {
  return [p.left != null ? `left:${p.left}px` : "", p.right != null ? `right:${p.right}px` : "", p.top != null ? `top:${p.top}px` : "", p.bottom != null ? `bottom:${p.bottom}px` : ""].join(";");
}

// ---------- B · isla ----------
function islandB(st, { notch = true, top = 0, mode = S.mode, gallery = false } = {}) {
  if (st === "idle") return gallery ? `<div class="statusline" style="padding:10px">Sin llamada: la isla desaparece (el notch queda como siempre).</div>` : "";
  const W = 1280;
  const open = mode === "expanded";
  const peek = st === "speaking" && !open;
  const w = open ? 400 : peek ? 420 : notch ? 330 : 250;
  const h = open ? 190 : 32;
  const radius = notch ? `0 0 ${open ? 24 : 14}px ${open ? 24 : 14}px` : `${open ? 24 : 16}px`;
  const pos = gallery ? `position:relative;width:${w}px;height:${h}px;border-radius:${radius}` : `left:${(W - w) / 2}px;top:${top}px;width:${w}px;height:${h}px;border-radius:${radius}`;
  if (!open) {
    return `<div class="island st-${st}" style="${pos}" data-act="expand">
      <div class="row">${markBox(st)}${peek ? `<span class="peek">${esc(conv().a)}: «Listo, ya compila…»</span>` : ""}</div>
      <div class="rw">${st === "muted" ? `<span style="color:#ff8a80;display:flex;width:16px">${I.micOff}</span>` : st === "reconnecting" ? `<span style="color:var(--muted)">…</span>` : ""}<span>${clock()}</span></div>
    </div>`;
  }
  return `<div class="island open st-${st}" style="${pos}">
    <div class="row">${markBox(st)}<div style="flex:1;min-width:0"><div class="title">${esc(conv().t)}</div><div class="statusline">${esc(stateText(st))} · ${clock()}</div></div></div>
    <div class="ctrls">
      ${micBtn(st)}
      <button class="iconbtn" data-act="skip" title="Saltar respuesta (S)" ${st === "speaking" ? "" : "disabled"}>${I.skip}</button>
      <button class="chip" data-act="nextconv" title="Siguiente conversación">${esc(CONVS[(S.conv + 1) % CONVS.length].t)} →</button>
      <button class="iconbtn" data-act="openapp" title="Abrir Sidevoice">${I.open}</button>
      ${hangBtn(st)}
    </div>
  </div>`;
}

// ---------- C · barra ----------
function trayMac(st, open) {
  const txt = st === "idle" ? "" : st === "muted" ? "Silenciado" : clock();
  const slash = st === "muted" ? `<svg viewBox="0 0 24 24" style="position:absolute;width:16px;height:16px"><line x1="3" y1="3" x2="21" y2="21" stroke="#fff" stroke-width="2.2" stroke-linecap="round"/></svg>` : "";
  const withText = S.option === "C";
  return `<span class="traymark ${open ? "open" : ""}" data-act="tray" style="position:relative;cursor:pointer"><span style="position:relative;display:flex">${MARK_WHITE}${slash}</span>${st !== "idle" && !withText ? `<span class="dot"></span>` : ""}${withText && txt ? `<span class="t">${txt}</span>` : ""}</span>`;
}
function popoverC(st, pos, { arrowLeft = 280, arrow = true, gallery = false } = {}) {
  const style = gallery ? "position:relative" : posStyle(pos);
  if (st === "idle") {
    return `<div class="popover hud ${arrow ? "" : "noarrow"}" style="${style}"><style>.popover::before{left:${arrowLeft}px}</style>
      <div class="head">${markBox("idle")}<div><div class="title">Sidevoice</div><div class="statusline">Sin llamada</div></div></div>
      ${convList()}<div class="actions"><button class="chip primary" data-act="openapp">Abrir para llamar</button><button class="chip">Ajustes…</button></div></div>`;
  }
  return `<div class="popover hud st-${st} ${arrow ? "" : "noarrow"}" style="${style}"><style>.popover::before{left:${arrowLeft}px}</style>
    <div class="head">${markBox(st)}<div style="flex:1;min-width:0"><div class="title">${esc(conv().t)}</div><div class="statusline">${esc(stateText(st))}</div></div><span class="timer">${clock()}</span></div>
    <div class="ctrls">
      <button class="chip ${st === "muted" ? "primary" : ""}" data-act="mute">${st === "muted" ? "Activar micro" : "Silenciar"}</button>
      <button class="chip" data-act="skip" ${st === "speaking" ? "" : "disabled"}>Saltar</button>
      <button class="chip" data-act="hang" style="border-color:var(--danger);color:#ff8a80">Colgar</button>
    </div>
    ${convList()}
    <div class="actions"><button class="chip" data-act="openapp">Abrir Sidevoice</button><button class="chip">Ajustes…</button></div>
    ${keysLine()}
  </div>`;
}
function nativeMenu(st, pos) {
  const c = conv();
  return `<div class="natmenu" style="${posStyle(pos)}">
    <div class="dis">${st === "idle" ? "Sin llamada" : `${esc(c.t)} — ${esc(stateText(st, true))} · ${clock()}`}</div><hr>
    <div data-act="mute">${st === "muted" ? "Activar micrófono" : "Silenciar micrófono"}</div>
    <div data-act="skip" class="${st === "speaking" ? "" : "dis"}">Saltar respuesta</div>
    <div data-act="hang">Colgar</div><hr>
    <div>Conversación <span>▸</span></div><hr>
    <div data-act="openapp">Mostrar Sidevoice</div><div>Ajustes…</div><hr><div>Salir de Sidevoice</div>
  </div>`;
}
function thumbWin(st, pos) {
  return `<div class="thumb" style="${posStyle(pos)}">
    <div class="ttl"><span style="width:14px;height:14px;display:flex">${MARK}</span>Sidevoice — ${esc(conv().t)}</div>
    <div class="shot st-${st}"><div style="width:44px;height:44px;position:relative">${MARK}${st === "muted" ? SLASH : ""}</div></div>
    <div class="tbtns"><span title="Silenciar" style="${st === "muted" ? "background:var(--danger)" : ""}">${st === "muted" ? I.micOff : I.mic}</span><span title="Saltar">${I.skip}</span><span title="Colgar" style="color:#ff8a80">${I.hang}</span></div>
  </div>`;
}

// ---------- D · sistema ----------
function sysMedia(st, kind, pos, gallery = false) {
  const style = gallery ? "position:relative" : posStyle(pos);
  const playing = st !== "muted";
  const t2 = st === "idle" ? "—" : `${conv().a} · ${stateText(st, true)} · ${clock()}`;
  return `<div class="sysmedia ${kind}" style="${style}">
    <div class="row"><div class="art st-${st}">${MARK}</div><div style="min-width:0"><div class="t1">${esc(conv().t)}</div><div class="t2">${esc(t2)}</div><div class="t2" style="opacity:.5">Sidevoice</div></div></div>
    <div class="pb"><span>⏮<em>anterior</em></span><span>${playing ? "⏸" : "▶︎"}<em>${playing ? "silenciar" : "activar micro"}</em></span><span>⏭<em>siguiente</em></span></div>
    <div style="height:16px"></div>
  </div>`;
}

// ---------- escritorio ----------
function editor(x, y, w, h) {
  return `<div class="editor" style="left:${x}px;top:${y}px;width:${w}px;height:${h}px">
    <div class="tbar"><i style="background:#ff5f57"></i><i style="background:#febc2e"></i><i style="background:#28c840"></i><span style="margin-left:10px">tray.rs — sidevoice-desktop</span></div>
    <div class="code"><span class="c">//! The menu-bar (tray) icon: call state at a glance, mute, hang up.</span>
<span class="k">pub fn</span> <span class="f">update</span>(app: &amp;AppHandle, snapshot: &amp;CallSnapshot) {
    <span class="k">let</span> Some(items) = app.try_state::&lt;TrayItems&gt;() <span class="k">else</span> { <span class="k">return</span> };
    <span class="k">let</span> view = bridge::tray_view(snapshot);
    <span class="k">let</span> _ = items.status.set_text(&amp;view.status);
    <span class="k">let</span> _ = items.mute.set_text(view.mute_label);
    <span class="c">// la ventana del editor sigue con el foco: la mini llamada no se lo quita</span>
    <span class="k">let</span> _ = items.hang_up.set_enabled(view.hang_up_enabled);
}</div></div>`;
}

function macDesktop(st, notch) {
  const open = S.option === "C" && S.mode === "expanded";
  let surface = "", hints = "";
  if (S.option === "A") {
    const pos = cornerPos(32, 0);
    surface = pillA(st, pos);
    hints = hint(pos.left != null ? 380 : 540, pos.top != null ? 150 : 560, "NSPanel no activante (tauri-nspanel): encima de todo, también de apps a pantalla completa, en todos los Escritorios. Pulsar no quita el foco al editor.");
  } else if (S.option === "B") {
    surface = notch ? islandB(st, { notch: true, top: 0 }) : islandB(st, { notch: false, top: 38 });
    hints = hint(840, 240, notch ? "Ventana sin bordes a nivel de la barra de menús, centrada sobre el notch (NSScreen.auxiliaryTopLeftArea). Pasa el ratón para desplegar." : "Mac sin notch o pantalla externa: la misma isla, suelta bajo la barra.");
  } else if (S.option === "C") {
    surface = open ? popoverC(st, { right: 136, top: 38 }) : "";
    hints = hint(820, 420, "NSStatusItem con título (TrayIcon::set_title): tiempo o «Silenciado» junto a la marca. Pulsar abre el panel anclado (ventana sin bordes que se cierra al perder el foco).");
  } else {
    surface = sysMedia(st, "mac", { right: 16, top: 44 });
    hints = hint(560, 90, "Centro de control → «Ahora suena». MPNowPlayingInfoCenter + MPRemoteCommandCenter (ya lo usa la app para los auriculares). Sin rojo, sin botón de colgar: los botones son los de un reproductor.");
  }
  return `<div class="desk mac">
    <div class="menubar"><span class="apple"></span><span class="app">Code</span><span>File</span><span>Edit</span><span>Selection</span><span>View</span>
      <span class="right">${st !== "idle" ? `<span class="orange" title="Micrófono en uso (lo pone macOS)"></span>` : ""}${trayMac(st, open)}<span>Wi‑Fi</span><span>🔋</span><span>mié 30 sept 18:42</span></span></div>
    ${notch ? `<div class="notch"></div>` : ""}
    ${editor(40, 70, 1000, 660)}
    ${surface}${hints}
  </div>`;
}

function winDesktop(st) {
  const open = S.mode === "expanded";
  let surface = "", hints = "";
  if (S.option === "A") {
    const pos = cornerPos(0, 48);
    surface = pillA(st, pos);
    hints = hint(430, 150, "Ventana siempre encima, tipo herramienta (WS_EX_TOOLWINDOW + WS_EX_NOACTIVATE): sin botón en la barra de tareas y sin quitar el foco.");
  } else if (S.option === "B") {
    surface = islandB(st, { notch: false, top: 8 });
    hints = hint(860, 120, "Sin notch en Windows: píldora arriba en el centro, siempre encima. Mismo componente que en el Mac.");
  } else if (S.option === "C") {
    surface = (open ? popoverC(st, { right: 12, bottom: 60 }, { arrow: false }) : "") + (S.thumb && !open ? thumbWin(st, { left: 560, bottom: 56 }) : "");
    hints = hint(40, 90, "Windows no admite texto en la bandeja: el estado va en el icono y en su tooltip. Clic → panel sobre la bandeja. Pasar por el botón de la barra de tareas → miniatura con Silenciar/Saltar/Colgar (ITaskbarList3, como Teams).");
  } else {
    surface = sysMedia(st, "win", { right: 12, bottom: 60 });
    hints = hint(40, 90, "Panel multimedia de Windows (SMTC). WebView2 lo alimenta desde navigator.mediaSession de la web; la app lo usa como mando. Mismos límites que en el Mac: botones de reproductor.");
  }
  const trayIcon = `<span data-act="tray" style="display:flex;position:relative;cursor:pointer" title="Sidevoice — ${esc(stateText(st, true))}">${st === "idle" ? MARK_WHITE : `<span class="st-${st}" style="display:flex;width:16px;height:16px">${MARK}</span>`}${st === "muted" ? SLASH : ""}</span>`;
  return `<div class="desk win">
    ${editor(60, 30, 1000, 660)}
    ${surface}${hints}
    <div class="taskbar">
      <div class="tb"><div class="sq" style="background:linear-gradient(135deg,#4cc2ff,#0078d4)"></div></div>
      <div class="tb"><div class="sq" style="background:#1e1f24;border:1px solid #444"></div></div>
      <div class="tb active"><div class="sq" style="background:#23a9f2"></div></div>
      ${S.option === "A" || S.option === "B" ? "" : `<div class="tb active" data-act="thumb" style="cursor:pointer" title="Sidevoice"><div class="sq" style="background:var(--berenjena);display:grid;place-items:center"><span style="width:14px;height:14px;display:flex">${MARK}</span></div></div>`}
      <div class="tray">${st !== "idle" ? `<span class="mic" title="Micrófono en uso (Windows 11)">🎙</span>` : ""}${trayIcon}<span>📶</span><span>🔊</span><div class="clock">18:42<br>30/09/2026</div></div>
    </div>
  </div>`;
}

function linuxDesktop(st, wayland) {
  let surface = "", hints = "";
  if (S.option === "A") {
    if (wayland) {
      surface = pillA(st, { left: 470, top: 330 });
      hints = hint(430, 440, "Wayland (GNOME): la app no puede colocarse ni ponerse encima. La píldora sale donde la ponga el compositor; la persona la fija una vez con Alt+Espacio → «Siempre encima». Alternativa: arrancar en XWayland (GDK_BACKEND=x11) y se comporta como en X11.");
    } else {
      const pos = cornerPos(30, 0);
      surface = pillA(st, pos);
      hints = hint(430, 150, "X11 (o XWayland): siempre encima y en la esquina, como en Windows. KDE en Wayland: la misma limitación que GNOME salvo con una regla de ventana de KWin.");
    }
  } else if (S.option === "B") {
    surface = wayland ? "" : islandB(st, { notch: false, top: 38 });
    hints = hint(420, 300, wayland ? "Wayland: no hay forma de fijar una píldora arriba en el centro sin protocolos del compositor (layer-shell: KDE sí, GNOME no). En GNOME/Wayland esta opción cae a la A." : "X11: píldora arriba en el centro, como en Windows.");
  } else if (S.option === "C") {
    surface = S.mode === "expanded" ? nativeMenu(st, { right: 90, top: 34 }) : "";
    hints = hint(420, 300, "Bandeja en Linux = AppIndicator: el clic abre SIEMPRE un menú nativo; no hay clic izquierdo propio ni panel anclado. GNOME necesita la extensión AppIndicator (Ubuntu la trae). Texto junto al icono: sí con AppIndicator (set_title), no en todos los paneles.");
  } else {
    surface = sysMedia(st, "gnome", { left: 470, top: 40 });
    hints = hint(830, 90, "MPRIS: el reproductor aparece en el panel de fecha/notificaciones de GNOME y en el de KDE. WebKitGTK debería publicarlo desde Media Session (sin verificar).");
  }
  return `<div class="desk gnome">
    <div class="gbar"><span class="acts">Actividades</span><span class="clock">30 sept 18:42</span>
      <span class="right">${st !== "idle" ? `<span title="Micrófono en uso (GNOME)">🎙</span>` : ""}<span data-act="tray" style="display:flex;position:relative;cursor:pointer" class="st-${st}">${st === "idle" ? MARK_WHITE : `<span style="display:flex;width:16px;height:16px">${MARK}</span>`}${st === "muted" ? SLASH : ""}</span><span>🔊</span><span>⏻</span></span></div>
    ${editor(70, 60, 1000, 690)}
    ${surface}${hints}
  </div>`;
}

function cornerPos(topBar, bottomBar) {
  const m = 12;
  switch (S.corner) {
    case "tl": return { left: m, top: topBar + m };
    case "bl": return { left: m, bottom: bottomBar + m + (S.mode === "expanded" ? 300 : 0) };
    case "br": return { right: m, bottom: bottomBar + m + (S.mode === "expanded" ? 300 : 0) };
    default: return { right: m, top: topBar + m };
  }
}
function hint(x, y, text) {
  return S.hints ? `<div class="hint" style="left:${x}px;top:${y}px">${esc(text)}</div>` : "";
}

// ---------- galería de estados ----------
function gallery() {
  const plat = isMac() ? "mac" : S.platform === "win" ? "win" : "gnome";
  const cells = STATE_ORDER.map((st) => {
    let html;
    if (S.option === "A") html = pillA(st, {}, { mode: "compact", gallery: true });
    else if (S.option === "B") html = islandB(st, { notch: S.platform === "mac", mode: "compact", gallery: true });
    else if (S.option === "C") html = `<div style="background:rgba(20,22,30,.9);padding:6px 10px;border-radius:6px;display:inline-flex">${trayMac(st, false)}</div>`;
    else html = sysMedia(st, plat, {}, true);
    return `<div class="cell"><div class="mini">${html}</div><div class="cap">${esc(stateText(st))}</div></div>`;
  });
  let extra = "";
  if (S.option === "A") {
    extra = `<div class="cell"><div class="mini">${pillA("speaking", {}, { mode: "expanded", gallery: true })}</div><div class="cap">Desplegada (hover, clic o ${mod()}Espacio)</div></div>
      <div class="cell"><div class="mini" style="display:flex;gap:10px">${pillA("listening", {}, { mode: "hidden", gallery: true })}${pillA("muted", {}, { mode: "hidden", gallery: true })}</div><div class="cap">Escondida al borde (${mod()}V): solo la marca; punto rojo si estás silenciado</div></div>`;
  } else if (S.option === "B") {
    extra = `<div class="cell"><div class="mini">${islandB("listening", { notch: S.platform === "mac", mode: "expanded", gallery: true })}</div><div class="cap">Desplegada</div></div>`;
  } else if (S.option === "C") {
    extra = `<div class="cell"><div class="mini">${popoverC("speaking", {}, { gallery: true, arrow: false })}</div><div class="cap">Panel al pulsar el icono</div></div>`;
  }
  return cells.join("") + extra;
}

// ---------- ficha de la opción ----------
const NOTES = {
  A: {
    how: [
      "Una segunda ventana de Tauri (<code>mini</code>) con el mismo puente que la bandeja: lee el <code>CallSnapshot</code> y manda <code>Command</code>s a la ventana de la sala. No carga otra web: es una página local pequeña.",
      "macOS: <b>tauri-nspanel</b> la convierte en NSPanel no activante (nivel flotante, todos los Escritorios, encima de pantalla completa). Sin él, <code>always_on_top</code> + <code>visible_on_all_workspaces</code> + <code>focusable(false)</code> se acercan pero no pisan la pantalla completa.",
      "Windows: <code>always_on_top</code>, <code>skip_taskbar</code>, <code>decorations(false)</code>, <code>transparent</code>, <code>focusable(false)</code>; WS_EX_NOACTIVATE si hace falta con el crate <code>windows</code>.",
      "Linux: igual en X11. En Wayland ni posición ni «encima»: se abre como ventana normal pequeña y la app enseña cómo fijarla (o XWayland).",
    ],
    pros: ["El núcleo funciona igual en los tres sistemas.", "Se ve el estado sin buscarlo: quién habla, silenciado, tiempo.", "Es la forma que ya conocen de Meet/Teams/Zoom.", "La isla (B) puede ser después solo otra forma de esta misma ventana."],
    cons: ["Una ventana más encima de su trabajo: por eso se esconde al borde y recuerda la esquina.", "Wayland/GNOME: la persona tiene que fijarla una vez.", "Transparencia en macOS pide <code>macos-private-api</code> (bien fuera de la App Store)."],
    effort: "M",
  },
  B: {
    how: [
      "La misma ventana que A, sin bordes, centrada arriba: en un Mac con notch, a nivel de la barra de menús y fundida con él (como NotchNook o Boring Notch). Crece al pasar el ratón o cuando habla el agente.",
      "Mac sin notch y pantallas externas: píldora bajo la barra. Windows y Linux X11: arriba en el centro.",
      "Linux Wayland: imposible sin layer-shell (KDE sí, GNOME no); cae a A.",
    ],
    pros: ["La más «nativa» en un MacBook: no ocupa sitio útil.", "Muy poco intrusiva cuando no pasa nada."],
    cons: ["Solo brilla en Macs con notch; en el resto es una píldora centrada.", "Tapar la barra de menús a nivel de sistema es frágil (Espacios, pantalla completa, varias pantallas) y Apple lo cambia entre versiones.", "Poco sitio para el título de la conversación."],
    effort: "M–L",
  },
  C: {
    how: [
      "Lo que ya existe (icono de la barra con menú) más: texto junto al icono en macOS (<code>set_title</code>), y un panel propio anclado al icono en vez del menú (ventana sin bordes colocada con la posición del icono; <code>tauri-plugin-positioner</code>).",
      "Windows: icono + tooltip (no admite texto) y panel sobre la bandeja; además, botones Silenciar/Saltar/Colgar en la miniatura de la barra de tareas (ITaskbarList3), como Teams.",
      "Linux: AppIndicator solo da menú nativo, sin clic propio: el menú crece (estado, silenciar, colgar, submenú de conversaciones).",
    ],
    pros: ["Cero ventanas encima del trabajo.", "Lo más barato: gran parte ya está hecha.", "Funciona en Wayland igual que en todos."],
    cons: ["No se ve quién habla sin mirar la barra; en Windows el icono es de 16 px.", "En Linux no hay panel, solo menú.", "Cambiar de conversación es un clic más lejos."],
    effort: "S–M",
  },
  D: {
    how: [
      "La app ya se hace «Ahora suena» en macOS para los botones del auricular. D lo lleva más lejos: título = conversación, subtítulo = estado; play/pausa = silenciar; siguiente/anterior = conversación.",
      "Windows: SMTC (panel multimedia) desde la Media Session de WebView2. Linux: MPRIS (GNOME y KDE lo muestran en el panel).",
      "Truco extra: una ventana Imagen en Imagen con un vídeo generado desde un canvas; en Chromium (WebView2) la Media Session tiene acciones de llamada (silenciar, colgar) en PiP; en WebKit (Mac, Linux) no.",
    ],
    pros: ["Integración de verdad con el sistema, sin ventana propia.", "Gratis con los auriculares y las teclas multimedia."],
    cons: ["Los botones son de reproductor: ⏸ significa «silenciar» y no hay colgar ni rojo.", "Choca con Spotify y compañía: solo hay un «Ahora suena».", "No es un mando de llamada: mejor como capa extra de cualquiera de las otras."],
    effort: "S (capa) · L (PiP)",
  },
};

function notes() {
  const n = NOTES[S.option];
  return `<h2>${esc(OPTIONS[S.option].name)} <span style="color:var(--muted);font-weight:400">· esfuerzo ${n.effort}</span></h2>
    <h3>Cómo se hace</h3><ul>${n.how.map((x) => `<li>${x}</li>`).join("")}</ul>
    <h3>A favor</h3><ul>${n.pros.map((x) => `<li>${x}</li>`).join("")}</ul>
    <h3>En contra</h3><ul>${n.cons.map((x) => `<li>${x}</li>`).join("")}</ul>`;
}

// ---------- render ----------
function render() {
  const st = S.state;
  const stage = $("#stage");
  let desk;
  if (S.platform === "mac") desk = macDesktop(st, true);
  else if (S.platform === "macnonotch") desk = macDesktop(st, false);
  else if (S.platform === "win") desk = winDesktop(st);
  else desk = linuxDesktop(st, S.platform === "wayland");
  stage.innerHTML = desk;
  $("#gallery").innerHTML = gallery();
  $("#notes").innerHTML = notes();
  $("#keys-mod").textContent = mod();
  document.querySelectorAll("[data-opt]").forEach((b) => b.setAttribute("aria-pressed", b.dataset.opt === S.option));
  document.querySelectorAll("[data-plat]").forEach((b) => b.setAttribute("aria-pressed", b.dataset.plat === S.platform));
  document.querySelectorAll("[data-state]").forEach((b) => b.setAttribute("aria-pressed", b.dataset.state === S.state));
  document.querySelectorAll("[data-mode]").forEach((b) => b.setAttribute("aria-pressed", b.dataset.mode === S.mode));
  document.querySelectorAll("[data-corner]").forEach((b) => b.setAttribute("aria-pressed", b.dataset.corner === S.corner));
  $("#corner-group").style.display = S.option === "A" ? "" : "none";
}
function fit() {
  const wrap = $("#stagewrap");
  $("#stage").style.transform = `scale(${wrap.clientWidth / 1280})`;
}
let toastTimer;
function toast(t) {
  const el = $("#toast");
  el.textContent = t;
  el.classList.add("on");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => el.classList.remove("on"), 1400);
}

// ---------- acciones (las mismas desde ratón y teclado) ----------
function act(a, arg) {
  const st = S.state;
  switch (a) {
    case "mute":
      if (st === "idle" || st === "reconnecting") return;
      if (st === "muted") { S.state = S.prevState === "muted" ? "listening" : S.prevState; toast("Micrófono activado"); }
      else { S.prevState = st; S.state = "muted"; toast("Silenciado"); }
      break;
    case "hang":
      if (st === "idle") return;
      if (S.armedHang > Date.now() || arg === "click") { S.state = "idle"; S.mode = "compact"; S.armedHang = 0; toast("Llamada terminada"); }
      else { S.armedHang = Date.now() + 1500; toast("Pulsa otra vez para colgar"); setTimeout(render, 1600); }
      break;
    case "skip":
      if (st === "speaking") { S.state = "listening"; toast("Respuesta saltada"); }
      break;
    case "expand":
      S.mode = S.mode === "expanded" ? "compact" : "expanded";
      break;
    case "hide":
      S.mode = S.mode === "hidden" ? "compact" : "hidden";
      break;
    case "tray":
      if (S.option === "C") S.mode = S.mode === "expanded" ? "compact" : "expanded";
      else toast("Menú de la barra (el de hoy)");
      break;
    case "thumb":
      S.thumb = !S.thumb;
      break;
    case "openapp":
      toast("Se abriría la ventana completa de Sidevoice");
      break;
    case "nextconv":
      setConv((S.conv + 1) % CONVS.length);
      break;
    case "conv":
      setConv(arg);
      break;
  }
  render();
}
function setConv(i) {
  if (!CONVS[i]) return;
  S.conv = i;
  CONVS[i].unread = 0;
  toast(`Conversación: ${CONVS[i].t}`);
}

document.addEventListener("click", (e) => {
  const c = e.target.closest("[data-conv]");
  if (c) { e.stopPropagation(); return act("conv", +c.dataset.conv); }
  const b = e.target.closest("[data-act]");
  if (b && !b.disabled && b.dataset.act) { e.stopPropagation(); return act(b.dataset.act, "click"); }
  const o = e.target.closest("[data-opt]");
  if (o) { S.option = o.dataset.opt; S.mode = "compact"; return render(); }
  const p = e.target.closest("[data-plat]");
  if (p) { S.platform = p.dataset.plat; return render(); }
  const s = e.target.closest("[data-state]");
  if (s) { S.state = s.dataset.state; if (S.state === "idle") S.start = Date.now(); return render(); }
  const m = e.target.closest("[data-mode]");
  if (m) { S.mode = m.dataset.mode; return render(); }
  const k = e.target.closest("[data-corner]");
  if (k) { S.corner = k.dataset.corner; return render(); }
  const h = e.target.closest("[data-hints]");
  if (h) { S.hints = !S.hints; h.setAttribute("aria-pressed", S.hints); return render(); }
});

// Atajos simulados: ⌃⌥ (Mac) / Ctrl+Alt (Windows, Linux). En el navegador se aceptan los dos.
let commandMode = 0;
document.addEventListener("keydown", (e) => {
  const chord = e.ctrlKey && e.altKey;
  const inCommand = commandMode > Date.now();
  if (chord) {
    const k = e.code;
    if (k === "KeyM") { e.preventDefault(); return act("mute"); }
    if (k === "KeyH") { e.preventDefault(); return act("hang"); }
    if (k === "KeyV") { e.preventDefault(); return act("hide"); }
    if (k === "Space") { e.preventDefault(); commandMode = Date.now() + 5000; S.mode = "expanded"; render(); return toast("Mando abierto: M, S, 1–9, H, Esc"); }
    if (k === "ArrowRight" || k === "ArrowDown") { e.preventDefault(); return act("nextconv"); }
    if (k === "ArrowLeft" || k === "ArrowUp") { e.preventDefault(); return act("conv", (S.conv + CONVS.length - 1) % CONVS.length); }
  }
  if (inCommand && !e.ctrlKey && !e.metaKey && !e.altKey) {
    commandMode = Date.now() + 5000;
    if (e.key === "Escape") { commandMode = 0; S.mode = "compact"; render(); return toast("Mando cerrado; el foco vuelve a tu app"); }
    if (/^[1-9]$/.test(e.key)) return act("conv", +e.key - 1);
    if (e.key === "m" || e.key === "M") return act("mute");
    if (e.key === "s" || e.key === "S") return act("skip");
    if (e.key === "h" || e.key === "H") return act("hang");
  }
});

S.hints = true;
// Enlaces directos: ?o=B&p=win&s=speaking&m=expanded&c=br&notas=0
{
  const q = new URLSearchParams(location.search);
  if (OPTIONS[q.get("o")]) S.option = q.get("o");
  if (["mac", "macnonotch", "win", "x11", "wayland"].includes(q.get("p"))) S.platform = q.get("p");
  if (STATES[q.get("s")]) S.state = q.get("s");
  if (["compact", "expanded", "hidden"].includes(q.get("m"))) S.mode = q.get("m");
  if (["tl", "tr", "bl", "br"].includes(q.get("c"))) S.corner = q.get("c");
  if (q.get("notas") === "0") S.hints = false;
  if (S.state === "muted") S.prevState = "listening";
}
window.addEventListener("resize", fit);
render();
fit();
setInterval(() => {
  // Solo refresca los relojes: el resto de la página no cambia sola.
  document.querySelectorAll(".timer, .traymark .t").forEach((el) => { if (/^\d/.test(el.textContent)) el.textContent = clock(); });
}, 1000);
