const source = await Deno.readTextFile(new URL("./app.js", import.meta.url));
const start = source.indexOf("function mergeChatLists");
const end = source.indexOf("\n\n(() =>", start);
const mergeChatLists = Function(`${source.slice(start, end)}; return mergeChatLists;`)();

const chat = (id, date, preview) => ({ id, last_date: date, last_preview: preview });
const assertEquals = (actual, expected) => {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) throw new Error(`expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
};

Deno.test("limited chat updates retain older chats and sort by latest date", () => {
  const existing = [chat(1, 100, "old conversation"), chat(2, 300, "selected")];
  const merged = mergeChatLists(existing, [chat(2, 400, "updated")]);
  assertEquals(merged.map(c => c.id), [2, 1]);
  assertEquals(merged[0].last_preview, "updated");
  assertEquals(merged.find(c => c.id === 1).last_preview, "old conversation");
});
