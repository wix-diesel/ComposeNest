"use strict";

// Environment state is illustrative and is discarded when the page reloads.
const icons = {
  nest: '<path d="m3 8 9-5 9 5-9 5zM3 12l9 5 9-5M3 16l9 5 9-5"/>',
  grid: '<rect x="3" y="3" width="7" height="7" rx="1.5"/><rect x="14" y="3" width="7" height="7" rx="1.5"/><rect x="3" y="14" width="7" height="7" rx="1.5"/><rect x="14" y="14" width="7" height="7" rx="1.5"/>',
  layers: '<path d="m3 7 9-4 9 4-9 4zM3 12l9 4 9-4M3 17l9 4 9-4"/>',
  archive: '<rect x="3" y="3" width="18" height="5" rx="1"/><path d="M5 8v12h14V8M10 12h4"/>',
  activity: '<path d="M2 12h5l3-8 4 16 3-8h5"/>',
  settings: '<path d="m9 3-1 3-3 1-2 3 2 2-1 3 2 3 3-1 2 3h3l1-3 3-1 2-3-2-2 1-3-2-3-3 1-2-3z"/><circle cx="11.5" cy="11.5" r="3"/>',
  home: '<path d="m3 10 9-7 9 7M5 9v12h14V9M9 21v-8h6v8"/>',
  play: '<path d="m8 4 12 8-12 8z"/>',
  stop: '<rect x="5" y="5" width="14" height="14" rx="2"/>',
  info: '<circle cx="12" cy="12" r="9"/><path d="M12 11v6M12 7v.1"/>',
  plus: '<path d="M12 5v14M5 12h14"/>',
  refresh: '<path d="M20 8a8 8 0 0 0-14-3L3 8M3 3v5h5M4 16a8 8 0 0 0 14 3l3-3M16 16h5v5"/>',
  search: '<circle cx="10.5" cy="10.5" r="6.5"/><path d="m16 16 5 5"/>',
  database: '<ellipse cx="12" cy="5" rx="8" ry="3"/><path d="M4 5v14c0 4 16 4 16 0V5M4 12c0 4 16 4 16 0"/>',
  arrow: '<path d="M5 12h14m-5-5 5 5-5 5"/>',
  copy: '<rect x="8" y="8" width="12" height="13" rx="2"/><path d="M16 8V3H3v13h5"/>',
  edit: '<path d="m14 4 6 6M4 14 16 2l6 6-12 12-7 1z"/>',
};
document.querySelectorAll("[data-icon]").forEach((element) => {
  element.innerHTML = icons[element.dataset.icon] || icons.info;
});

const toast = document.querySelector(".toast");
let toastTimer;
function notify(message) {
  clearTimeout(toastTimer);
  toast.textContent = message;
  toast.hidden = false;
  toastTimer = setTimeout(() => { toast.hidden = true; }, 4500);
}

const dialog = document.querySelector("dialog");
const confirmButton = document.querySelector("#dialog-confirm");
function showDialog(title, message, confirmLabel, onConfirm) {
  document.querySelector("#dialog-title").textContent = title;
  document.querySelector("#dialog-body").textContent = message;
  confirmButton.hidden = !confirmLabel;
  confirmButton.textContent = confirmLabel || "確認";
  confirmButton.onclick = () => { dialog.close(); onConfirm?.(); };
  dialog.showModal();
}

function refresh() {
  const time = document.querySelector("#observed-at");
  if (time) time.textContent = `最終確認 ${new Date().toLocaleTimeString("ja-JP")}`;
  notify("サンプルの確認時刻を更新しました。実際のDockerには接続していません。");
}

function selectTab(button) {
  document.querySelectorAll("[data-tab]").forEach((tab) => {
    const selected = tab === button;
    tab.classList.toggle("active", selected);
    tab.setAttribute("aria-selected", String(selected));
    tab.tabIndex = selected ? 0 : -1;
    document.getElementById(`panel-${tab.dataset.tab}`).hidden = !selected;
  });
}

let activeFilter = "all";
function filterInstances() {
  const query = document.querySelector("#instance-search").value.trim().toLowerCase();
  let visible = 0;
  document.querySelectorAll(".instance-card").forEach((card) => {
    card.hidden = !(activeFilter === "all" || activeFilter === card.dataset.state) || !card.dataset.name.toLowerCase().includes(query);
    if (!card.hidden) visible++;
  });
  document.querySelector("#empty-results").hidden = visible !== 0;
  document.dispatchEvent(new Event("instances-filtered"));
}
document.querySelector("#instance-search")?.addEventListener("input", filterInstances);

function updateSummary() {
  for (const key of ["version", "port", "storage"]) {
    const source = key === "storage" ? document.querySelector('input[name="storage"]:checked') : document.getElementById(key);
    const target = document.querySelector(`[data-summary="${key}"]`);
    if (source && target) target.textContent = source.value;
  }
  const description = document.querySelector("#storage-description");
  const mode = document.querySelector('input[name="storage"]:checked');
  if (description && mode) {
    description.textContent = mode.value === "bind mount"
      ? "管理ルート配下に新しい専用フォルダーを割り当てます。"
      : "Dockerが管理する新しい専用volumeを割り当てます。ホストのパス指定は不要です。";
  }
}
document.querySelector("#create-form")?.addEventListener("input", updateSummary);

let running = true;
function toggleRuntime(button) {
  running = !running;
  const state = document.querySelector("#runtime-state");
  state.textContent = running ? "利用可能" : "停止中";
  state.className = `badge ${running ? "ready" : ""}`;
  document.querySelector("#last-operation").textContent = `${running ? "起動" : "停止"} · 完了`;
  button.textContent = running ? "停止" : "起動";
  document.querySelector('[data-action="restart"]').disabled = !running;
  notify(`${running ? "起動" : "停止"}後のサンプル表示に切り替えました。`);
}

function operationResult(success) {
  document.querySelector("#progress-bar").style.width = success ? "100%" : "65%";
  const badge = document.querySelector("#operation-badge");
  badge.textContent = success ? "利用可能" : "失敗";
  badge.className = `badge ${success ? "ready" : "danger"}`;
  document.querySelector(".panel-title h2").textContent = success ? "環境を作成しました" : "準備完了を確認できませんでした";
  const steps = document.querySelectorAll(".timeline li");
  steps[3].querySelector("time").textContent = success ? "完了" : "タイムアウト";
  steps[4].querySelector("time").textContent = success ? "完了" : "保留";
  steps[3].querySelector("p").textContent = success ? "サービスの応答と構成の一致を確認しました。" : "コンテナが動作中の可能性があります。再確認が必要です。";
  for (const step of [steps[3], steps[4]]) {
    step.querySelector(".check-icon").textContent = success ? "✓" : "!";
    step.querySelector(".check-icon").classList.toggle("pending", !success);
  }
  const result = document.querySelector("#operation-result");
  result.replaceChildren();
  const notice = document.createElement("div");
  notice.className = `notice ${success ? "" : "warning"}`;
  notice.textContent = success ? "接続先：127.0.0.1:5436 · データベース：app · ユーザー：app（サンプル）" : "起動待ちがタイムアウトしました。現在の状態を再確認してから、同じ環境・保存領域・パスワードで再試行します。";
  result.append(notice);
  if (!success) {
    const retry = document.createElement("button");
    retry.type = "button";
    retry.className = "btn primary";
    retry.textContent = "状態を再確認して再試行";
    retry.onclick = () => showDialog("同じ環境で再試行", "前の処理の終了と現在の状態を確認した想定です。環境ID・保存領域・パスワードは変更しません。", "再試行の完了例を表示", () => operationResult(true));
    result.append(retry);
  }
}

function diagnose(offline) {
  const message = document.querySelector("#diagnosis-notice .notice");
  message.classList.toggle("warning", offline);
  message.querySelector("strong").textContent = offline ? "Docker Engineに接続できません" : "Dockerを利用できます";
  message.querySelector("p").textContent = offline ? "Dockerを起動して再確認してください。環境は「確認不能」となります。保存情報は引き続き閲覧できます。最終成功確認：14:32:08。" : "ローカルの接続先を固定して操作します。表示内容はモックの診断結果です。";
  const engine = document.querySelectorAll(".diagnostic-row")[2];
  engine.querySelector(".badge").textContent = offline ? "確認不能" : "確認済み";
  engine.querySelector(".badge").className = `badge ${offline ? "warn" : "ready"}`;
  engine.querySelector("p").textContent = offline ? "Dockerを起動してから再確認してください。" : "ローカルEngineに接続可能";
  engine.querySelector(".check-icon").textContent = offline ? "!" : "✓";
  document.querySelector(".engine-card").textContent = offline ? "Docker 確認不能" : "Docker 接続済み";
  const status = document.querySelector(".topbar-right > span:first-child");
  status.textContent = offline ? "接続を確認してください" : "ローカル接続";
}

const actions = {
  "close-dialog": () => dialog.close(),
  refresh,
  "reload-templates": () => notify("テンプレート2件を再読込みした想定です。実際のファイルは読み込みません。"),
  reveal: (button) => {
    const input = document.querySelector("#password");
    input.type = input.type === "password" ? "text" : "password";
    button.textContent = input.type === "password" ? "表示" : "隠す";
  },
  "detail-secret": (button) => {
    const revealed = button.getAttribute("aria-pressed") !== "true";
    document.querySelector("#detail-secret").textContent = revealed ? "MockOnly-7pQ2k9" : "••••••••••••••";
    button.setAttribute("aria-pressed", String(revealed));
    button.textContent = revealed ? "隠す" : "表示";
  },
  "compose-secret": (button) => {
    const revealed = button.getAttribute("aria-pressed") !== "true";
    const output = document.querySelector("#compose-output");
    output.textContent = output.textContent.replace(revealed ? "••••••••" : "MockOnly-7pQ2k9", revealed ? "MockOnly-7pQ2k9" : "••••••••");
    button.setAttribute("aria-pressed", String(revealed));
    button.textContent = revealed ? "秘密情報を隠す" : "原文を表示";
    const notice = document.querySelector("#panel-compose .notice strong");
    notice.textContent = revealed ? "秘密情報を含むサンプル原文を表示しています" : "秘密情報を隠して表示しています";
  },
  "toggle-runtime": toggleRuntime,
  restart: () => showDialog("環境を再起動", "開発用データベースを再起動します。一時的に接続が切れます。データと設定は保持されます。", "再起動", () => notify("再起動が完了した想定です。Docker操作は行っていません。")),
  delete: () => showDialog("環境を削除。データは残ります", "対象：開発用データベース\nコンテナと専用ネットワークを削除し、環境一覧から外します。\n保持するデータ：C:\\ProgramData\\ComposeNest\\instances\\cn-7f2a\\data\n元の設定とComposeも残します。", "データを残して削除", () => notify("削除確認が完了しました。モックのためデータや画面は変更しません。")),
  follow: (button) => {
    const following = button.getAttribute("aria-pressed") !== "true";
    button.setAttribute("aria-pressed", String(following));
    button.textContent = following ? "追従を停止" : "追従を再開";
    notify(following ? "ログ追従の表示を再開しました（サンプル）。" : "ログ追従の表示を停止しました。環境は停止しません。");
  },
  "reload-logs": () => notify("サンプルログを再表示しました。実際のログ購読は行っていません。"),
  "operation-success": () => operationResult(true),
  "operation-failure": () => operationResult(false),
  offline: () => diagnose(true),
  diagnose: () => { diagnose(false); refresh(); },
};

document.addEventListener("click", async (event) => {
  const button = event.target.closest("button");
  if (!button) return;
  if (button.dataset.notice) notify(button.dataset.notice);
  if (button.dataset.action) actions[button.dataset.action]?.(button);
  if (button.dataset.tab) selectTab(button);
  if (button.dataset.filter) {
    activeFilter = button.dataset.filter;
    document.querySelectorAll("[data-filter]").forEach((item) => {
      item.classList.toggle("active", item === button);
      item.setAttribute("aria-pressed", String(item === button));
    });
    filterInstances();
  }
  if (button.dataset.copy) {
    try {
      await navigator.clipboard.writeText(button.dataset.copy);
      notify("サンプル値をコピーしました。");
    } catch {
      showDialog("コピーできませんでした", "ブラウザでクリップボードへのアクセスが許可されていません。画面上の値を選択してコピーしてください。");
    }
  }
});

document.querySelector('[role="tablist"]')?.addEventListener("keydown", (event) => {
  const tabs = [...document.querySelectorAll("[data-tab]")];
  const index = tabs.indexOf(document.activeElement);
  if (index < 0 || !["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
  event.preventDefault();
  const next = event.key === "Home" ? 0 : event.key === "End" ? tabs.length - 1 : (index + (event.key === "ArrowRight" ? 1 : -1) + tabs.length) % tabs.length;
  selectTab(tabs[next]);
  tabs[next].focus();
});

for (const id of ["create-form", "clone-form", "edit-form", "settings-form"]) {
  document.getElementById(id)?.addEventListener("submit", (event) => {
    event.preventDefault();
    if (id === "settings-form") { notify("保存方式をこの画面内で保存しました。保存方式は再読込みすると元に戻ります。テーマは別途保存されます。"); return; }
    if (id === "create-form") {
      const name = document.querySelector("#environment-name").value.trim();
      if (!name) { notify("環境名を入力してください。"); return; }
      showDialog("新しい環境を作成", `${name}\nPostgreSQL ${document.querySelector("#version").value} / ポート ${document.querySelector("#port").value}\n保存方式：${document.querySelector('input[name="storage"]:checked').value}\n設定と資源を再検証してから作成する想定です。`, "作成", () => notify("作成を受け付けた想定です。進捗の画面例はoperation.htmlにあります。"));
    } else if (id === "clone-form") {
      const name = document.querySelector("#clone-name").value.trim();
      if (!name) { notify("複製先の環境名を入力してください。"); return; }
      showDialog("設定を複製して作成", `${name}\nPostgreSQL 18 / ポート候補 5436\n保存方式：${document.querySelector('input[name="storage"]:checked').value}\nパスワードと保存領域を新しく生成します。元のデータは複製しません。`, "複製して作成", () => notify("複製を受け付けた想定です。元環境への変更はありません。"));
    } else {
      const name = document.querySelector("#edit-name").value.trim();
      if (!name) { notify("環境名を入力してください。"); return; }
      showDialog("変更内容の確認", `環境名：検証用データベース → ${name}\n接続ポート：5433 → ${document.querySelector("#edit-port").value}\nデータを保持し、適用後も停止状態を維持します。`, "変更を適用", () => notify("変更を適用した想定です。環境は停止したままです。"));
    }
  });
}
