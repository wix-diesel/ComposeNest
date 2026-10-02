"use strict";

const instanceCards = [...document.querySelectorAll(".instance-card")];
const gridBody = document.querySelector("#instance-grid tbody");
const gridRows = instanceCards.map((card) => {
  const row = document.createElement("tr");
  const name = card.querySelector("h3").textContent;
  const service = card.querySelector(".service-description").textContent.trim().replace(/\s+/g, " ");
  const metadata = [...card.querySelectorAll("dd")].map((item) => item.textContent.trim());
  const values = [name, service, "", ...metadata, ""];
  for (const value of values) {
    const cell = document.createElement("td");
    cell.textContent = value;
    row.append(cell);
  }
  row.children[0].classList.add("environment-name");
  row.children[2].append(card.querySelector(".badge").cloneNode(true));
  row.children[3].classList.add("mono");
  const detail = card.querySelector(".card-footer button").cloneNode(true);
  detail.setAttribute("aria-label", `${name}の詳細`);
  row.children[6].append(detail);
  row.dataset.name = name;
  row.dataset.port = metadata[0].split(":").pop();
  row.dataset.state = card.querySelector(".badge").textContent;
  gridBody.append(row);
  return { card, row };
});

function syncGridFilter() {
  let count = 0;
  for (const { card, row } of gridRows) {
    row.hidden = card.hidden;
    if (!row.hidden) count++;
  }
  document.querySelector("#result-count").textContent = `${count} / ${gridRows.length} 環境`;
}
document.addEventListener("instances-filtered", syncGridFilter);
syncGridFilter();

function setInstanceView(view) {
  const isGrid = view === "grid";
  document.querySelector(".cards").hidden = isGrid;
  document.querySelector("#instance-grid").hidden = !isGrid;
  document.querySelectorAll("[data-view]").forEach((button) => {
    const selected = button.dataset.view === view;
    button.classList.toggle("active", selected);
    button.setAttribute("aria-pressed", String(selected));
  });
}
document.querySelectorAll("[data-view]").forEach((button) => {
  button.addEventListener("click", () => {
    setInstanceView(button.dataset.view);
    if (!mockDisplayPreferences.write("view", button.dataset.view)) {
      notify("表示形式はこの画面内で適用しました。ブラウザの制限により保存できません。");
    }
  });
});
setInstanceView(mockDisplayPreferences.read("view") === "grid" ? "grid" : "cards");

document.querySelectorAll("[data-sort]").forEach((button) => {
  button.addEventListener("click", () => {
    const header = button.closest("th");
    const ascending = header.getAttribute("aria-sort") !== "ascending";
    document.querySelectorAll("[aria-sort]").forEach((item) => item.setAttribute("aria-sort", "none"));
    header.setAttribute("aria-sort", ascending ? "ascending" : "descending");
    const key = button.dataset.sort;
    const rows = [...gridRows].sort((a, b) => {
      const order = key === "port"
        ? Number(a.row.dataset.port) - Number(b.row.dataset.port)
        : a.row.dataset[key].localeCompare(b.row.dataset[key], "ja");
      return ascending ? order : -order;
    });
    rows.forEach(({ row }) => gridBody.append(row));
  });
});
