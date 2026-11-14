class YButton extends HTMLElement {
  static observedAttributes = ["disabled", "class"];
  role_ = "button";
  connectedCallback() {
    this.setAttribute("role", this.role_);
    this.sync();
    this.addEventListener("click", (e) => {
      if (this.disabled) {
        e.stopImmediatePropagation();
        e.preventDefault();
      }
    }, true);
    this.addEventListener("keydown", (e) => {
      if ((e.key === "Enter" || e.key === " ") && !this.disabled) {
        e.preventDefault();
        this.click();
      }
    });
  }
  attributeChangedCallback() {
    this.sync();
  }
  sync() {
    this.tabIndex = this.disabled ? -1 : 0;
    this.setAttribute("aria-disabled", String(this.disabled));
  }
  get disabled() {
    return this.hasAttribute("disabled");
  }
  set disabled(v) {
    this.toggleAttribute("disabled", !!v);
  }
}

class YTab extends YButton {
  role_ = "tab";
  sync() {
    super.sync();
    this.setAttribute("aria-selected", String(this.classList.contains("on")));
  }
}

customElements.define("y-button", YButton);
customElements.define("y-tab", YTab);
