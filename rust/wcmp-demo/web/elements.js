// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

// The custom element classes of the demo. A class has to extend
// `HTMLElement` in JavaScript, which Rust cannot write, so the page's
// Rust calls `demoElements.define` with a tag, the attributes the tag
// observes, and its own three callbacks, and this file writes the
// class. Each instance opens its shadow root in its constructor, and
// hands each lifecycle callback to Rust, which calls the tag's
// component. Rust finds an element's state through the element itself.
// The shadow root starts with a slot, so that an element shows its
// children until its first render replaces the slot with its view.

window.demoElements = {
  define(tag, observed, host) {
    customElements.define(
      tag,
      class extends HTMLElement {
        static get observedAttributes() {
          return observed;
        }

        constructor() {
          super();
          this.attachShadow({ mode: "open" }).append(document.createElement("slot"));
        }

        connectedCallback() {
          host.connected(this);
        }

        disconnectedCallback() {
          host.disconnected(this);
        }

        attributeChangedCallback(name, before, after) {
          if (before !== after) {
            host.attributeChanged(this, name, after);
          }
        }
      },
    );
  },
};
