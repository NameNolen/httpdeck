client.test("脚本文件：状态码为 200", function () {
  client.assert(response.status === 200, "期望 200，实际 " + response.status);
});
client.log("来自脚本文件，状态码:", response.status);
