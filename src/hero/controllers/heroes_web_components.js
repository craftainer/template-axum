// Progressive enhancement for /heroes/form (FR-0034): every form still
// works with this script absent (a plain HTML POST/redirect round trip
// against controllers::heroes_web's own handlers). Once loaded, this
// intercepts the same forms and submits them with fetch instead, to the same
// endpoints and with the same HttpOnly session cookie (credentials:
// "same-origin"), so a create/update/delete no longer needs a full page
// navigation and a validation error shows inline. No token is read or kept
// here: the page never contains one and script cannot read the cookie.
(function () {
  "use strict";

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

  function submit(form) {
    // The form's own method and action, with its fields urlencoded exactly as
    // a plain submission would send them. `redirect: "manual"` stops fetch
    // from following the 303 back to the page: success is an opaque redirect.
    var body = new URLSearchParams(new FormData(form));
    return fetch(form.action, {
      method: form.method.toUpperCase(),
      body: body,
      credentials: "same-origin",
      redirect: "manual",
    }).then(function (response) {
      if (response.status === 422) {
        return response.json().then(function (problem) {
          showErrors(form, problem.detail);
          var invalid = new Error("validation failed");
          invalid.shown = true;
          throw invalid;
        });
      }
      if (response.type !== "opaqueredirect" && !response.ok) {
        throw new Error("request failed: " + response.status);
      }
      clearErrors(form);
      return response;
    });
  }

  document.addEventListener("submit", function (event) {
    var form = event.target;
    if (
      !form.classList ||
      !(form.classList.contains("hero-form") || form.classList.contains("hero-delete-form"))
    ) {
      return;
    }
    event.preventDefault();
    submit(form)
      .then(function () {
        window.location.reload();
      })
      .catch(function (err) {
        // Field errors are already shown inline by submit(). Any other
        // failure (an expired session, a refused role, the rate limit) is
        // handed to the browser as the plain form post it would have been
        // without this script, so the server's own error page is shown
        // rather than nothing.
        if (!err || !err.shown) form.submit();
      });
  });
})();
