"""Drive the dashboard in Chrome against the demo realm: sign in via Keycloak, use every page,
screenshot each. Run with the Compose stack up: pip install playwright && playwright install chromium
&& python e2e/ui_test.py"""
import os
import sys
from playwright.sync_api import sync_playwright, expect

BASE = os.environ.get("BASE", "http://localhost:3100")
import os
SHOTS = os.environ.get("SHOTS", "screenshots")
os.makedirs(SHOTS, exist_ok=True)

def sign_in(page, user):
    page.goto(f"{BASE}/")
    page.get_by_role("link", name="Sign in to Cloud Cache").click()
    page.fill("#username", user)
    if page.locator("#password").count() == 0:
        page.locator("input[type=submit], button[type=submit]").first.click()
    page.fill("#password", user)
    page.locator("input[type=submit], button[type=submit]").first.click()
    page.wait_for_url(f"{BASE}/keys**")

with sync_playwright() as p:
    browser = p.chromium.launch(args=["--no-sandbox"])
    ctx = browser.new_context(viewport={"width": 1360, "height": 860})
    page = ctx.new_page()
    page.goto(f"{BASE}/"); page.screenshot(path=f"{SHOTS}/01-signin.png")

    sign_in(page, "alice")
    page.screenshot(path=f"{SHOTS}/02-keys-empty.png")

    # Create keys through the editor
    for k, v, ttl in [("session:42", "signed in", ""), ("user:1", '{"name":"Ada"}', ""), ("config", "mode=fast", "3600")]:
        page.goto(f"{BASE}/keys?new=1")
        page.fill("#key", k); page.fill("#value", v)
        if ttl: page.fill("#ttl", ttl)
        page.get_by_role("button", name="Create key").click()
        expect(page.get_by_text(f"Saved {k}")).to_be_visible()
    page.goto(f"{BASE}/keys")
    rows = page.locator("tbody tr")
    print("keys listed:", rows.count(), [rows.nth(i).locator("td").first.inner_text() for i in range(rows.count())])

    page.get_by_role("link", name="session:42").click()
    expect(page.locator("#value")).to_have_value("signed in")
    page.fill("#value", "signed in (edited)")
    page.get_by_role("button", name="Save changes").click()
    expect(page.get_by_text("Saved session:42")).to_be_visible()
    page.screenshot(path=f"{SHOTS}/03-keys-edit.png")

    page.goto(f"{BASE}/keys?prefix=user:")
    print("prefix filter:", page.locator("tbody tr").count(), "row(s)")

    page.goto(f"{BASE}/keys?key=config")
    page.on("dialog", lambda d: d.accept())
    page.get_by_role("button", name="Delete key").click()
    page.wait_for_url("**deleted=config**")
    print("after delete:", page.locator("tbody tr").count(), "rows;", page.get_by_role("status").inner_text())

    page.goto(f"{BASE}/usage"); page.screenshot(path=f"{SHOTS}/04-usage.png", full_page=True)
    print("usage readouts:", [x.inner_text() for x in page.locator(".readout").all()])

    page.goto(f"{BASE}/connect"); page.screenshot(path=f"{SHOTS}/05-connect.png", full_page=True)

    page.goto(f"{BASE}/members")
    print("members:", [x.inner_text().split("\n")[0] for x in page.locator("li p.font-semibold").all()])
    page.fill("#email", "carol@globex.example"); page.get_by_role("button", name="Add member").click()
    expect(page.get_by_text("Added carol@globex.example")).to_be_visible()
    page.fill("#email", "nobody@nowhere.example"); page.get_by_role("button", name="Add member").click()
    expect(page.get_by_text("No account uses nobody@nowhere.example")).to_be_visible()
    carol = page.locator("li", has_text="carol@globex.example")
    carol.get_by_role("button", name="Make admin").click()
    expect(carol.get_by_text("Admin", exact=True)).to_be_visible()
    page.screenshot(path=f"{SHOTS}/06-members.png", full_page=True)
    carol.get_by_role("button", name="Remove admin").click()
    expect(carol.get_by_role("button", name="Make admin")).to_be_visible()
    carol.get_by_role("button", name="Remove member").click()
    expect(page.locator("li", has_text="carol@globex.example")).to_have_count(0)
    print("carol added, promoted, demoted, removed: ok")


    # Bob: read-only member, not admin
    ctx2 = browser.new_context(viewport={"width": 1360, "height": 860}); p2 = ctx2.new_page()
    sign_in(p2, "bob")
    print("bob sees keys:", p2.locator("tbody tr").count(), "| New key button:", p2.get_by_role("link", name="New key").count(),
          "| Members nav:", p2.get_by_role("link", name="Members").count())
    p2.goto(f"{BASE}/members"); print("bob /members ->", p2.url)

    # Dave: two orgs, switches between them
    ctx3 = browser.new_context(viewport={"width": 1360, "height": 860}); p3 = ctx3.new_page()
    sign_in(p3, "dave")
    print("dave starts in:", p3.locator("aside p.text-2xl").inner_text(), "| keys:", p3.locator("tbody tr").count())
    p3.select_option("#org-switch", label="globex"); p3.wait_for_load_state("networkidle")
    print("dave switched to:", p3.locator("aside p.text-2xl").inner_text(), "| keys:", p3.locator("tbody tr").count())
    p3.screenshot(path=f"{SHOTS}/08-dave-globex.png")

    # Mobile layout
    m = browser.new_context(viewport={"width": 390, "height": 844}, storage_state=ctx.storage_state()).new_page()
    m.goto(f"{BASE}/keys"); m.screenshot(path=f"{SHOTS}/09-mobile-keys.png", full_page=True)

    # Sign out ends the Keycloak session too
    page.goto(f"{BASE}/keys"); page.get_by_role("button", name="Sign out").click()
    page.wait_for_url(f"{BASE}/"); page.goto(f"{BASE}/keys")
    print("after sign-out /keys ->", page.url.split("?")[0])
    browser.close()
