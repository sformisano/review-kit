"use strict";

(() => {
  const byId = (id) => document.getElementById(id);
  const text = (id, value) => { byId(id).textContent = value; };
  const element = (tag, className, content) => {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (content !== undefined) node.textContent = content;
    return node;
  };
  const lessons = [...document.querySelectorAll(".lesson")];
  const links = [...document.querySelectorAll(".desktop-lessons a")];
  const titles = ["Start with a finding", "Fix it. Keep F1.", "Choose the rules", "Make an assessment", "Publish the review", "Read the code"];

  function showLesson(moveFocus = false) {
    const requested = location.hash.slice(1);
    const index = Math.max(0, lessons.findIndex((lesson) => lesson.id === requested));
    lessons.forEach((lesson, i) => { lesson.hidden = i !== index; });
    links.forEach((link, i) => {
      if (i === index) link.setAttribute("aria-current", "step");
      else link.removeAttribute("aria-current");
    });
    byId("lesson-select").value = lessons[index].id;
    text("chapter-label", index < 4 ? "THE DOMAIN" : index === 4 ? "THE APPLICATION" : "THE SOURCE");
    text("lesson-count", `${String(index + 1).padStart(2, "0")} / 06`);
    const previous = byId("previous-lesson");
    const next = byId("next-lesson");
    previous.hidden = index === 0;
    if (index > 0) previous.href = `#${lessons[index - 1].id}`;
    next.hidden = index === lessons.length - 1;
    if (index < lessons.length - 1) {
      next.href = `#${lessons[index + 1].id}`;
      next.replaceChildren(document.createTextNode(`Next: ${titles[index + 1]} `), element("span", "", "→"));
      next.lastChild.setAttribute("aria-hidden", "true");
    }
    document.title = `${titles[index]} · Learn Review Kit`;
    if (moveFocus) {
      lessons[index].querySelector("h1").focus({ preventScroll: true });
      window.scrollTo({ top: 0, behavior: "instant" });
    }
  }

  function pressOnly(attribute, value) {
    document.querySelectorAll(`[${attribute}]`).forEach((button) => {
      button.setAttribute("aria-pressed", String(button.getAttribute(attribute) === value));
    });
  }

  function inspectFinding(part) {
    const example = learningExamples.findingParts[part];
    pressOnly("data-inspect", part);
    const value = element("div", "field-value");
    value.append(element("p", "object-label", example.type));
    const pre = element("pre");
    pre.append(element("code", "", example.value));
    value.append(pre);
    const explanation = element("div", "field-explanation");
    explanation.append(element("strong", "", example.heading), element("p", "", example.explanation));
    byId("finding-detail").replaceChildren(value, explanation);
  }

  function showConstruction() {
    const missing = byId("blank-evidence").checked;
    text("finding-construction-type", missing ? "Err" : "Finding");
    text("finding-construction-result", missing ? 'Rejected: FindingError::Blank("evidence"). No Finding is returned.' : "Accepted: title, body, and evidence are present.");
    byId("finding-construction-result").classList.toggle("rejected", missing);
  }

  function showLifecycle(state) {
    const example = learningExamples.lifecycle[state];
    pressOnly("data-lifecycle", state);
    text("current-revision", `at ${example.revision}`);
    text("current-finding-status", example.status);
    byId("current-finding-status").className = `status ${example.style}`;
    text("current-finding-evidence", example.evidence);
    text("lifecycle-foot", example.foot);
    text("transition-direction", example.direction);
    text("transition-explanation", example.explanation);
    text("finding-equality", String(example.equal));
    byId("finding-equality").className = `boolean ${example.equal}`;
    text("finding-equality-reason", example.equalityReason);
  }

  function showSelection() {
    const example = learningExamples.selections[byId("policy-request").value];
    const result = byId("selection-result");
    if (example.error) {
      const error = element("div", "selection-error");
      error.append(element("code", "", example.error), element("p", "", example.explanation));
      result.replaceChildren(error);
      return;
    }
    const heading = element("p", "selection-result-heading", "Assessment slots · worker-review");
    const rows = example.rows.map(([id, reason]) => {
      const row = element("div", "selection-row");
      row.append(element("code", "", id), element("span", "", reason));
      return row;
    });
    result.replaceChildren(heading, ...rows);
  }

  function evaluationRow(id, judgment, style) {
    const row = element("div", `evaluation-row ${style === "missing" ? "missing" : ""}`);
    row.append(element("code", "", id), element("span", style === "missing" ? "" : `status ${style}`, judgment));
    return row;
  }

  function showAssessment() {
    const example = learningExamples.assessments[byId("assessment-example").value];
    text("assessment-revision", `Revision ${example.revision}`);
    text("assessment-decision", example.decision);
    text("assessment-finding-status", example.status);
    byId("assessment-finding-status").className = `status ${example.style}`;
    byId("assessment-evaluations").replaceChildren(
      evaluationRow("core-review", "Satisfied", "satisfied"),
      evaluationRow("error-handling", example.evaluation, example.evaluationStyle),
      evaluationRow("test-evidence", example.missingEvaluation ? "Missing" : "Satisfied", example.missingEvaluation ? "missing" : "satisfied"),
      element("p", "evaluation-explanation", example.evidence)
    );
    byId("assessment-limitation").hidden = !example.limitation;
    text("assessment-limitation", example.limitation || "");
    text("assessment-result-title", example.accepted ? "Accepted" : "Rejected");
    text("verdict-mark", example.accepted ? "✓" : "×");
    document.querySelector(".constructor-verdict").classList.toggle("rejected", !example.accepted);
    const detail = byId("assessment-result-detail");
    if (example.accepted) detail.replaceChildren(element("p", "", example.explanation));
    else {
      const errors = element("ul");
      example.errors.forEach(([reason, name]) => {
        const item = element("li", "", reason);
        item.append(element("code", "error-name", name));
        errors.append(item);
      });
      detail.replaceChildren(errors);
    }
  }

  let publicationStep = 0;
  function showPublication() {
    const stages = learningExamples.publications[byId("publication-example").value];
    const stage = stages[publicationStep];
    const trace = stages.map((item, index) => {
      const row = element("li", index < publicationStep ? "past" : index === publicationStep ? "current" : "", item.label);
      row.append(element("small", "", item.actor));
      if (index === publicationStep) row.setAttribute("aria-current", "step");
      return row;
    });
    byId("publication-trace").replaceChildren(...trace);
    text("publication-stage-label", stage.actor);
    text("publication-stage-title", stage.title);
    text("publication-stage-description", stage.description);
    text("publication-observation", stage.observation);
    text("publication-lesson", stage.lesson);
    document.querySelector(".observed-state").classList.toggle("unknown", !!stage.unknown);
    text("publication-step-count", `Step ${publicationStep + 1} of ${stages.length}`);
    byId("publication-back").disabled = publicationStep === 0;
    byId("publication-next").disabled = publicationStep === stages.length - 1;
    text("publication-next", publicationStep === stages.length - 1 ? "End of case" : "Next step →");
  }

  document.querySelectorAll("[data-inspect]").forEach((button) => button.addEventListener("click", () => inspectFinding(button.dataset.inspect)));
  document.querySelectorAll("[data-lifecycle]").forEach((button) => button.addEventListener("click", () => showLifecycle(button.dataset.lifecycle)));
  byId("blank-evidence").addEventListener("change", showConstruction);
  byId("policy-request").addEventListener("change", showSelection);
  byId("assessment-example").addEventListener("change", showAssessment);
  byId("publication-example").addEventListener("change", () => { publicationStep = 0; showPublication(); });
  byId("publication-back").addEventListener("click", () => { publicationStep--; showPublication(); });
  byId("publication-next").addEventListener("click", () => { publicationStep++; showPublication(); });
  byId("lesson-select").addEventListener("change", (event) => { location.hash = event.target.value; });
  window.addEventListener("hashchange", () => showLesson(true));
  inspectFinding("identity");
  showConstruction();
  showLifecycle("open");
  showSelection();
  showAssessment();
  showPublication();
  document.body.classList.add("has-js");
  showLesson();
})();
