# StaticFlow Self-Hosted systemd Quick Start

这是当前推荐的自托管部署方式：仓库脚本负责构建、切流和运维入口，`systemd` 负责长期托管进程，Pingora gateway 负责对外端口和 blue/green backend 切换。

```mermaid
flowchart LR
    A["clone repo"] --> B["build frontend"]
    B --> C["prepare release bundle"]
    C --> D["write env and gateway yaml"]
    D --> E["render systemd units"]
    E --> F["start blue and green backends"]
    F --> G["start gateway"]
    G --> H["status health switch logs"]
```

## 1. 适用范围

- Linux 主机，且可用 `systemd`
- 希望用固定 gateway 端口对外服务
- 希望 backend 按 `blue` / `green` 两个槽位运行，并通过脚本切流

如果你只是本地开发，继续用仓库里的前台脚本就够了；这份文档讲的是长期运行的 service 版本，不是 trunk 热重载开发模式。

## 2. 推荐目录布局

```text
/opt/staticflow/current                 仓库 checkout
/opt/staticflow/releases/current        release bundle
/etc/staticflow/selfhosted/             env 和 gateway YAML
/var/log/staticflow/runtime/            运行期日志目录
```

这里有两个目录角色，不要混淆：

- 仓库 checkout 提供 `scripts/`、模板和日常运维命令
- release bundle 提供编译好的 backend binary、gateway binary 和 `frontend/dist`

下面默认你把仓库放在 `/opt/staticflow/current`。

## 3. 准备依赖

```bash
cd /opt/staticflow
git clone <your-staticflow-repo-url> current
cd /opt/staticflow/current
git submodule update --init --recursive

rustup target add wasm32-unknown-unknown
cargo install trunk
```

你还需要主机上已有这些基础工具：

- `git`
- `make`
- `cargo`
- `npm`
- `python3`
- `systemctl`

## 4. 构建前端

```bash
cd /opt/staticflow/current
./scripts/build_frontend_selfhosted.sh
```

这一步会生成 `frontend/dist`，供后面的 release bundle 使用。

## 5. 生成 release bundle

```bash
cd /opt/staticflow/current
./scripts/prepare_selfhosted_systemd_bundle.sh \
  --output-dir /opt/staticflow/releases/current
```

完成后，你会得到这几个关键产物：

- `/opt/staticflow/releases/current/bin/static-flow-backend`
- `/opt/staticflow/releases/current/bin/staticflow-pingora-gateway`
- `/opt/staticflow/releases/current/frontend/dist`
- `/opt/staticflow/releases/current/conf/pingora/staticflow-gateway.yaml.template`

## 6. 写配置文件

先准备配置目录：

```bash
sudo mkdir -p /etc/staticflow/selfhosted
```

复制 env 模板：

```bash
cd /opt/staticflow/current

sudo cp deployment-examples/systemd/staticflow-common.env.example \
  /etc/staticflow/selfhosted/common.env
sudo cp deployment-examples/systemd/staticflow-gateway.env.example \
  /etc/staticflow/selfhosted/gateway.env
sudo cp deployment-examples/systemd/staticflow-backend-slot.env.example \
  /etc/staticflow/selfhosted/backend-slot-blue.env
sudo cp deployment-examples/systemd/staticflow-backend-slot.env.example \
  /etc/staticflow/selfhosted/backend-slot-green.env
sudo cp /opt/staticflow/releases/current/conf/pingora/staticflow-gateway.yaml.template \
  /etc/staticflow/selfhosted/pingora-gateway.yaml
```

至少要改这几项：

### `/etc/staticflow/selfhosted/common.env`

- `DB_ROOT`：你的数据根目录
- `FRONTEND_DIST_DIR`：指向 `/opt/staticflow/releases/current/frontend/dist`
- `BACKEND_BIN`：指向 `/opt/staticflow/releases/current/bin/static-flow-backend`
- `GATEWAY_BIN`：指向 `/opt/staticflow/releases/current/bin/staticflow-pingora-gateway`
- `SITE_BASE_URL`：你的站点地址

### `/etc/staticflow/selfhosted/pingora-gateway.yaml`

- `staticflow.listen_addr`：gateway 对外监听地址，比如 `127.0.0.1:39180`
- `staticflow.upstreams.blue`：blue backend 监听地址，比如 `127.0.0.1:39080`
- `staticflow.upstreams.green`：green backend 监听地址，比如 `127.0.0.1:39081`
- `staticflow.active_upstream`：初始流量打到 `blue` 还是 `green`

### `/etc/staticflow/selfhosted/backend-slot-blue.env`

通常不需要改太多；默认会从 gateway YAML 里解析 blue 的 host/port。需要的话可以单独覆盖：

- `STATICFLOW_LOG_SERVICE`
- `ADMIN_LOCAL_ONLY`

### `/etc/staticflow/selfhosted/backend-slot-green.env`

和 blue 同理；默认会从 gateway YAML 里解析 green 的 host/port。

## 7. 渲染 systemd units

```bash
cd /opt/staticflow/current
sudo ./scripts/render_selfhosted_systemd_units.sh \
  --unit-dir /etc/systemd/system \
  --workdir /opt/staticflow/current \
  --common-env /etc/staticflow/selfhosted/common.env \
  --gateway-env /etc/staticflow/selfhosted/gateway.env \
  --backend-env-pattern /etc/staticflow/selfhosted/backend-slot-%i.env
```

这里的 `--workdir` 必须指向仓库 checkout，而不是 release bundle，因为 service 实际上是通过仓库里的脚本做统一管理。

## 8. 启动服务

先让 `systemd` 重新加载 unit：

```bash
sudo systemctl daemon-reload
```

再启动 backend 两个槽位和 gateway：

```bash
sudo systemctl enable --now staticflow-backend-slot@blue.service
sudo systemctl enable --now staticflow-backend-slot@green.service
sudo systemctl enable --now staticflow-gateway.service
```

如果你只想先启一个槽位，也可以先只启动 `blue`。但推荐 blue 和 green 都常驻，这样后续切流更直接。

## 9. 验证服务

推荐优先用仓库脚本做运维检查：

```bash
cd /opt/staticflow/current

SYSTEMD_SCOPE=system \
CONF_FILE=/etc/staticflow/selfhosted/pingora-gateway.yaml \
./scripts/pingora_gateway.sh status

SYSTEMD_SCOPE=system \
CONF_FILE=/etc/staticflow/selfhosted/pingora-gateway.yaml \
./scripts/pingora_gateway.sh health

SYSTEMD_SCOPE=system \
./scripts/pingora_gateway.sh logs gateway --lines 100
```

也可以直接探活：

```bash
curl http://127.0.0.1:39180/api/healthz
```

如果返回 JSON 且 `port` 指向你预期的 blue 或 green 端口，说明 gateway 到 backend 的链路已经通了。

## 10. 日常运维

### 查看整体状态

```bash
cd /opt/staticflow/current
SYSTEMD_SCOPE=system \
CONF_FILE=/etc/staticflow/selfhosted/pingora-gateway.yaml \
./scripts/pingora_gateway.sh status
```

### 查看单个 service 状态

```bash
SYSTEMD_SCOPE=system ./scripts/pingora_gateway.sh status gateway
SYSTEMD_SCOPE=system ./scripts/pingora_gateway.sh status blue
SYSTEMD_SCOPE=system ./scripts/pingora_gateway.sh status green
```

### 看日志

```bash
SYSTEMD_SCOPE=system ./scripts/pingora_gateway.sh logs gateway --follow
SYSTEMD_SCOPE=system ./scripts/pingora_gateway.sh logs blue --lines 200
SYSTEMD_SCOPE=system ./scripts/pingora_gateway.sh logs green --lines 200
```

### 切流

```bash
SYSTEMD_SCOPE=system \
CONF_FILE=/etc/staticflow/selfhosted/pingora-gateway.yaml \
./scripts/pingora_gateway.sh switch green
```

切回 blue 只要把 `green` 换成 `blue`。

### 重启 backend 槽位

```bash
SYSTEMD_SCOPE=system ./scripts/pingora_gateway.sh restart-backend blue
SYSTEMD_SCOPE=system ./scripts/pingora_gateway.sh restart-backend green
```

### 重启 gateway

```bash
SYSTEMD_SCOPE=system ./scripts/pingora_gateway.sh restart
```

## 11. 一次标准升级流程

当你更新仓库后，推荐流程是：

1. 在仓库 checkout 里更新代码
2. 重新构建前端
3. 重新生成 release bundle 到同一个输出目录
4. 重启当前不在线的 backend 槽位
5. 用 `switch` 把 gateway 切到新槽位
6. 确认健康后，再处理旧槽位

这条路径的核心原则是：对外监听端口不变，gateway 统一收口，backend 用 blue/green 切换。

## 12. 只想先做隔离验证

如果你还不想动正式路径，只想验证这套模板和脚本能不能工作，可以直接跑：

```bash
cd /opt/staticflow/current
./scripts/test_selfhosted_systemd_stack.sh
```

它会用 `systemd --user` 和随机端口做一套隔离测试，不会直接占用你正式的 gateway/backend 端口。
