// Progressive enhancement for /heroes/form (FR-0034): every form still
// works with this script absent (a plain HTML POST/redirect round trip
// against controllers::heroes_web's own handlers). Once loaded, this
// intercepts the same forms and drives the existing JSON API
// (controllers::heroes, /crud/v1/heroes/v2/json) via fetch instead, so a
// create/update/delete no longer needs a full page navigation.
(function () {
  "use strict";

  var token = document.body.getAttribute("data-token") || "";

  function clearErrors(form) {
    var existing = form.querySelector(".errors");
    if (existing) existing.remove();
  }

  function showErrors(form, errors) {
    clearErrors(form);
    var list = document.createElement("ul");
    list.className = "errors";
    (errors || []).forEach(function (err) {
      var item = document.createElement("li");
      item.textContent = err.field + ": " + err.msg;
      list.appendChild(item);
    });
    form.prepend(list);
  }

  function payloadFrom(form) {
    var powersRaw = form.elements.powers ? form.elements.powers.value : "";
    var powers = powersRaw
      .split(",")
      .map(function (p) { return p.trim(); })
      .filter(function (p) { return p.length > 0; });
    var payload = {};
    if (form.elements.name) payload.name = form.elements.name.value;
    if (form.elements.powers) payload.powers = powers;
    if (form.elements.power_level) {
      var raw = form.elements.power_level.value.trim();
      payload.power_level = raw === "" ? null : Number(raw);
    }
    return payload;
  }

  function submit(form) {
    var url = form.getAttribute("data-json-action");
    var method = form.getAttribute("data-json-method");
    var body = method === "DELETE" ? undefined : JSON.stringify(payloadFrom(form));
    var headers = { "Authorization": "Bearer " + token };
    if (body !== undefined) headers["Content-Type"] = "application/json";

    return fetch(url, { method: method, headers: headers, body: body }).then(function (response) {
      if (response.status === 422) {
        return response.json().then(function (problem) {
          showErrors(form, problem.detail);
          throw new Error("validation failed");
        });
      }
      if (!response.ok) {
        throw new Error("request failed: " + response.status);
      }
      clearErrors(form);
      return response;
    });
  }

  document.addEventListener("submit", function (event) {
    var form = event.target;
    if (!form.classList || !form.classList.contains("hero-form") && !form.classList.contains("hero-delete-form")) {
      return;
    }
    if (!form.hasAttribute("data-json-action")) return;
    event.preventDefault();
    submit(form)
      .then(function () {
        window.location.reload();
      })
      .catch(function () {
        // Field errors are already shown inline by submit(); any other
        // failure leaves the form as-is rather than losing the caller's
        // input to a full-page error navigation.
      });
  });
})();
