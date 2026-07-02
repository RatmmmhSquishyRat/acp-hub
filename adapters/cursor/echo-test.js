const fs = require("fs");
fs.writeFileSync("C:/tmp/echo-adapter.log", "STARTED\n");
const rl = require("readline").createInterface({ input: process.stdin });
rl.on("line", (line) => {
  fs.appendFileSync("C:/tmp/echo-adapter.log", "RECV: " + line.slice(0,300) + "\n");
  let msg; try { msg = JSON.parse(line); } catch { return; }
  if (msg.method === "initialize") {
    process.stdout.write(JSON.stringify({jsonrpc:"2.0",id:msg.id,result:{protocolVersion:1,agentCapabilities:{loadSession:true,sessionCapabilities:{list:{},close:{},delete:{},resume:{mode:"none"}}}}}) + "\n");
  } else if (msg.method === "session/list") {
    const sessions = [{sessionId:"test-1",title:"Echo Test",cwd:"/tmp",additionalDirectories:[],updatedAt:"2026-06-28T00:00:00Z",meta:{}}];
    process.stdout.write(JSON.stringify({jsonrpc:"2.0",id:msg.id,result:{sessions:sessions,nextCursor:null}}) + "\n");
    fs.appendFileSync("C:/tmp/echo-adapter.log", "SENT " + sessions.length + " sessions\n");
  } else if (msg.id) {
    process.stdout.write(JSON.stringify({jsonrpc:"2.0",id:msg.id,result:{}}) + "\n");
  }
});
