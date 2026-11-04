"""Trusted, network-denied Playwright runner for authoring observations."""

from __future__ import annotations

import asyncio
import base64
import json
from pathlib import Path
from urllib.parse import urlsplit

from playwright.async_api import async_playwright

ROOT = Path("/tmp")  # noqa: S108 -- fixed private files in a fresh, single-use OpenSandbox.
ORIGIN = "https://preview.agentx.invalid"


async def run():
    request = json.loads((ROOT / "agentx-preview-input.json").read_text())
    options = request["options"]
    data = json.dumps(request, ensure_ascii=True).replace("<", r"\u003c")
    html = (
        '<!doctype html><html><head><meta charset="utf-8">'
        '<meta http-equiv="Content-Security-Policy" content="worker-src blob:">'
        '<link rel="stylesheet" href="/preview.css"></head><body>'
        '<div id="root"></div><script>window.agentxDesignInput=' + data + ";</script>"
        '<script src="/preview.js"></script></body></html>'
    )
    errors = []
    results = []
    async with async_playwright() as playwright:
        browser = await playwright.chromium.launch(headless=True, args=["--disable-dev-shm-usage"])
        context = await browser.new_context(
            viewport={"width": 1440, "height": 900},
            device_scale_factor=1,
            locale=options["locale"],
            color_scheme=options["theme"],
        )

        async def route_request(route):
            url = urlsplit(route.request.url)
            if f"{url.scheme}://{url.netloc}" != ORIGIN:
                await route.abort()
            elif url.path == "/":
                await route.fulfill(body=html, content_type="text/html")
            elif url.path in ("/preview.js", "/preview.css"):
                await route.fulfill(
                    path=str(ROOT / f"agentx-preview-{url.path[1:]}"),
                    content_type="text/javascript" if url.path.endswith(".js") else "text/css",
                )
            else:
                await route.abort()

        await context.route("**/*", route_request)
        page = await context.new_page()
        page.on("pageerror", lambda error: errors.append(str(error)[:500]))
        page.on("console", lambda message: errors.append(message.text[:500]) if message.type == "error" else None)
        await page.goto(ORIGIN, wait_until="load", timeout=15000)
        await page.wait_for_function("window.agentxDesignPreview?.ready", timeout=10000)
        await page.evaluate("window.agentxDesignPreview.inspect()")
        await page.wait_for_timeout(350)

        async def target_frame():
            if request["document"]["kind"] == "workflow":
                return page
            frame = page.frame(name="agentx-application-page")
            if frame is None:
                raise ValueError("The generated page frame is unavailable")
            return frame

        for index, action in enumerate(options["actions"]):
            try:
                kind = action["action"]
                frame = await target_frame()
                locator = frame.locator(action["selector"]) if "selector" in action else None
                if kind == "click":
                    await locator.click(timeout=3000)
                elif kind == "fill":
                    await locator.fill(action["value"], timeout=3000)
                elif kind == "select":
                    await locator.select_option(action["value"], timeout=3000)
                elif kind == "press":
                    await locator.press(action["key"], timeout=3000)
                elif kind == "upload":
                    await locator.set_input_files(
                        {"name": action["name"], "mimeType": "text/plain", "buffer": action["content"].encode()},
                        timeout=3000,
                    )
                elif kind == "wait":
                    await page.wait_for_timeout(action["milliseconds"])
                elif kind == "reload":
                    await page.evaluate("window.agentxDesignPreview.reload()")
                    await page.evaluate("window.agentxDesignPreview.inspect()")
                elif kind == "focus_nodes":
                    await page.evaluate("ids => window.agentxDesignPreview.focusNodes(ids)", action["nodeIds"])
                    await page.wait_for_timeout(150)
                elif kind == "check":
                    results.append({"index": index, "check": await page.evaluate("window.agentxDesignPreview.check()")})
                    continue
                results.append({"index": index, "action": kind, "status": "completed"})
            except Exception as error:
                results.append(
                    {"index": index, "action": action["action"], "status": "failed", "error": str(error)[:800]}
                )
                break
        frame = await target_frame()
        dom = await frame.locator("body").evaluate("""body => ({
          text: body.innerText.slice(0,12000),
          elements: [...body.querySelectorAll('input,textarea,select,button,a,[role="button"]')].slice(0,120).map(e=>({tag:e.tagName.toLowerCase(),id:e.id,name:e.getAttribute('name'),type:e.getAttribute('type'),action:e.getAttribute('data-action'),field:e.getAttribute('data-field-path'),text:(e.innerText||e.getAttribute('aria-label')||'').slice(0,180),value:e.value??null,visible:e.getClientRects().length>0,disabled:!!e.disabled})),
          nodes: [...body.querySelectorAll('.react-flow__node')].slice(0,200).map(e=>{const b=e.getBoundingClientRect();return{id:e.getAttribute('data-id'),text:e.innerText.slice(0,300),x:b.x,y:b.y,width:b.width,height:b.height}})
        })""")
        report = {
            "source": "isolated_browser",
            "actions": results,
            "errors": errors[:20],
            "dom": dom,
            "facts": await page.evaluate("window.agentxDesignPreview.facts()"),
        }
        image = None
        if options["screenshot"]:
            region = options.get("region")
            raw = (
                await frame.locator(region).screenshot(type="png", timeout=3000)
                if region
                else await page.screenshot(type="png")
            )
            image = await page.evaluate(
                """async data => {
              const img=new Image();img.src='data:image/png;base64,'+data;await img.decode();
              const scale=Math.min(1,1440/Math.max(img.width,img.height));
              const canvas=document.createElement('canvas');canvas.width=Math.max(1,Math.round(img.width*scale));canvas.height=Math.max(1,Math.round(img.height*scale));
              canvas.getContext('2d').drawImage(img,0,0,canvas.width,canvas.height);
              return {width:canvas.width,height:canvas.height,base64:canvas.toDataURL('image/webp',0.83).split(',')[1]};
            }""",
                base64.b64encode(raw).decode(),
            )
            if len(image["base64"]) > 768 * 1024:
                image = None
                report["errors"].append("Screenshot exceeds the bounded image size; request a smaller region")
        await context.close()
        await browser.close()
    print(json.dumps({"report": report, "image": image}, ensure_ascii=True))


if __name__ == "__main__":
    asyncio.run(run())
