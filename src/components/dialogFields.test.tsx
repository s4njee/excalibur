// What the New Connection dialog's fields tell the webview (E3-S5).
//
// The browser preview could not show this bug. WKWebView capitalises,
// autocorrects and spell-checks a text input by default, so typing `root` into
// Username produced `Root`; the server refused it, and the failure arrived
// looking like a network error. None of these fields is prose.
//
// The test enumerates whatever inputs the dialog renders rather than naming
// them, because the failure mode is a *new* field added without the
// attributes — exactly the field nobody would remember to add a case for.

import { describe, expect, it, afterEach, beforeEach } from "vitest";
import { render } from "solid-js/web";
import { ConnectionDialog } from "./ConnectionDialog";
import { fixtureBackend } from "../state/backend";

let host: HTMLDivElement;
let dispose: (() => void) | undefined;

const settle = () => new Promise((done) => setTimeout(done, 0));

/** Every field a user types into. Checkboxes and radios have nothing to fix. */
const typedFields = () =>
  [...host.querySelectorAll("input")].filter(
    (input) => input.type !== "checkbox" && input.type !== "radio",
  );

/** Switch the dialog to a protocol, which is what changes the field set. */
const chooseProtocol = async (label: string) => {
  const tab = [...host.querySelectorAll("button")].find(
    (button) => button.textContent?.trim() === label,
  );
  tab?.click();
  await settle();
};

beforeEach(async () => {
  host = document.createElement("div");
  document.body.append(host);
  dispose = render(
    () => (
      <ConnectionDialog
        backend={fixtureBackend()}
        onCancel={() => undefined}
        onConnected={() => undefined}
      />
    ),
    host,
  );
  await settle();
});

afterEach(() => {
  dispose?.();
  dispose = undefined;
  host.remove();
});

describe("the connection dialog's fields", () => {
  // Every protocol, because the dialog swaps the field set rather than
  // disabling what does not apply: Password and Domain exist only on some
  // tabs, and a tab this loop skipped would be a tab nothing covers.
  for (const protocol of ["SMB", "NFS", "SFTP", "xsync"]) {
    it(`asks the webview to leave them alone on ${protocol}`, async () => {
      await chooseProtocol(protocol);
      const fields = typedFields();
      expect(fields.length, `${protocol} rendered no fields`).toBeGreaterThan(0);
      for (const field of fields) {
        const name = `${protocol}/${field.id || field.type}`;
        expect(field.getAttribute("autocapitalize"), name).toBe("none");
        expect(field.getAttribute("autocorrect"), name).toBe("off");
        // The attribute rather than the IDL property: jsdom does not
        // implement `spellcheck`, and it is the attribute WebKit reads.
        expect(field.getAttribute("spellcheck"), name).toBe("false");
        // Something, always: an unset `autocomplete` lets the browser guess,
        // and its guess for a field called "share" is a street address.
        expect(field.getAttribute("autocomplete"), name).toBeTruthy();
      }
    });
  }

  it("offers no completion for the password, and the right one for the account", async () => {
    await chooseProtocol("SMB");
    const password = host.querySelector<HTMLInputElement>("#password");
    const username = host.querySelector<HTMLInputElement>("#username");
    // A password manager may hold the account name; the secret belongs in the
    // OS keychain through the "Save in keychain" box, not in a browser store.
    expect(password?.getAttribute("autocomplete")).toBe("off");
    expect(username?.getAttribute("autocomplete")).toBe("username");
  });
});
