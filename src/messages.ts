interface MessageStatus {
  state: string;
  detail: string;
  lastReport: {
    failures: { path: string; action: string; error: string }[];
    aborted: string | null;
  } | null;
}

// Also protects messages from an older running backend during an update.
function safeMessage(text: string): string {
  return text.replace(/https?:\/\/[^\s)]+/gi, "[service URL hidden]");
}

export function setupMessages(): (status: MessageStatus) => void {
  const dialog = document.createElement("dialog");
  dialog.id = "messages-dialog";
  dialog.className = "messages-dialog";
  dialog.setAttribute("aria-labelledby", "messages-title");
  dialog.innerHTML = `
    <div class="messages-heading"><h2 id="messages-title">Sync messages</h2>
      <form method="dialog"><button class="btn btn-sm" autofocus>Close</button></form></div>
    <p id="messages-summary" class="muted" role="status"></p>
    <ul id="messages-list" class="failures"></ul>`;
  document.body.append(dialog);
  const button = document.getElementById("btn-messages");
  const list = dialog.querySelector("ul");
  const summary = dialog.querySelector("#messages-summary");
  if (!button || !list || !summary) throw new Error("Missing sync message controls");
  button.addEventListener("click", () => dialog.showModal());
  dialog.addEventListener("close", () => button.focus());
  let previous = "";
  return (status) => {
    const needsAttention = ["offline", "error", "signInRequired"].includes(status.state);
    const failures = status.lastReport?.failures ?? [];
    const aborted = status.lastReport?.aborted;
    const key = JSON.stringify([needsAttention ? status.detail : "", status.state, failures, aborted]);
    if (key === previous) return;
    previous = key;
    const count = failures.length || Number(needsAttention || !!aborted);
    button.textContent = count ? `View messages (${count})` : "View messages";
    button.classList.toggle("btn-warning", count > 0);
    list.replaceChildren();
    const add = (text: string): void => {
      const item = document.createElement("li");
      item.textContent = text;
      list.append(item);
    };
    if (needsAttention) add(safeMessage(status.detail));
    for (const failure of failures) {
      add(`${failure.action} — ${failure.path}: ${safeMessage(failure.error)}`);
    }
    if (aborted) add(`Stopped early: ${safeMessage(aborted)}`);
    summary.textContent = status.state === "signInRequired"
      ? "Sign in again to continue syncing. Completed transfers have been saved."
      : count
        ? "Completed transfers have been saved. Unfinished items are checked again on the next sync."
        : "No sync problems to report.";
  };
}
