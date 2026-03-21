---
title: "WSL 存储扩容与 ext4 数据盘落地：从 DrvFs 卡顿到 data-4tb.vhdx 挂载方案"
summary: "一次围绕 StaticFlow 数据库存储路径的完整运维复盘：为什么 `/mnt/e` 上的 DrvFs 不适合 LanceDB，为什么没有直接改动现有 Ubuntu 根盘，如何在 `E:` 上创建独立 `ext4` 数据 VHDX，并把它稳定挂到 WSL 的 `/mnt/wsl/data4tb` 供业务使用。"
tags:
  - wsl
  - ext4
  - drvfs
  - vhdx
  - lancedb
  - storage
  - operations
  - windows
category: "Infrastructure"
category_description: "WSL storage architecture, capacity planning, and operational playbooks for database workloads."
author: "ackingliu"
date: "2026-03-22"
---

# WSL 存储扩容与 ext4 数据盘落地：从 DrvFs 卡顿到 data-4tb.vhdx 挂载方案

## 1. 背景与目标

这次折腾 WSL 存储，不是从“磁盘空间不够”开始的，而是从一个更具体的运行时症状开始的：

- StaticFlow 的音乐库和内容库最初都放在 `/mnt/e/static-flow-data`
- 这个路径在 WSL 里对应的是 Windows `E:` 盘，经由 **DrvFs / 9p** 暴露给 Linux
- 在后台 compaction 运行时，歌曲读取、音频 blob 访问这类操作会出现明显卡顿，甚至表现为“像卡死一样”

问题的关键不只是“性能慢”，而是数据库和大对象读写都在依赖一层并不等价于 Linux 原生 ext4 的文件系统语义。对于 LanceDB 这种带 manifest、fragment、blob、compaction、rename 和 metadata 交互的工作负载，这种差异会直接影响稳定性。

这次落地目标最终收敛成了四条：

1. 让 StaticFlow 的数据库路径脱离 `/mnt/e` 这类 DrvFs 路径。
2. 不动现有 Ubuntu 的系统盘 `D:\wsl_ubuntu\ext4.vhdx`。
3. 数据物理上仍然放在 `E:`，避免继续挤压当前发行版根盘。
4. 在 WSL 内部获得真正的 ext4 语义，而不是“看起来像本地盘”的 Windows 映射目录。

---

## 2. 模型与术语

为了避免后面概念混淆，这里先固定几个术语。

### 2.1 发行版根盘

发行版根盘是 Ubuntu 自己的那块系统盘，在这台机器上对应：

- Windows 文件：`D:\wsl_ubuntu\ext4.vhdx`
- WSL 内部挂载：`/`
- 文件系统类型：`ext4`

这是 WSL 发行版本体所在的磁盘。`/home`、`/var`、`/srv` 这些路径都属于这块盘。

### 2.2 DrvFs 路径

DrvFs 路径指的是：

- `/mnt/c`
- `/mnt/d`
- `/mnt/e`

这些路径的底层不是 Linux 本地 ext4，而是 Windows 文件系统通过 WSL 做的映射层。对普通源码目录、文档、共享文件来说它们很方便，但对数据库负载来说不是理想选择。

### 2.3 数据 VHDX

数据 VHDX 指的是新建的独立虚拟磁盘文件：

- `E:\wsl-disks\data-4tb.vhdx`

它的物理文件位于 `E:`，但挂入 WSL 之后会被格式化成独立的 ext4 文件系统，供业务数据使用。

### 2.4 业务数据根

最终业务使用的目录不是整块挂载盘根目录，而是这块盘下专门给业务准备的子目录：

- `/mnt/wsl/data4tb/static-flow-data`

这个目录由 `ts_user` 持有写权限，StaticFlow 后端最终指向这里。

---

## 3. 故障现象与约束条件

### 3.1 运行时症状

前期最明显的问题是：

- 后台 compaction 运行时，歌曲读取请求变慢
- `get_song_audio()` 这类读取 blob 的路径，和 compaction 同时打到 `/mnt/e` 时更容易出现“长期挂住”的观感
- 当前代码里并没有给 `songs` 表加应用层 shared/exclusive 锁，所以更像底层 IO 竞争，而不是新业务锁逻辑造成的阻塞

这个判断和存储层现状是一致的：

- `/mnt/e/static-flow-data` 不是 ext4
- 它走的是 Windows `E:` 盘的 DrvFs/9p 映射
- compaction 与 blob 读同时压这层映射时，更容易暴露吞吐下降和延迟拉长

### 3.2 容量约束

排查过程中又遇到了第二个现实条件：

- 当前 Ubuntu 根盘虽然是 ext4，但可用空间已经偏紧
- 根盘的 VHDX 大小约为 1TB 级别，而不是预期中的 2TB
- 即使 WSL 2.6.1 已支持 `wsl --manage Ubuntu --resize 2TB`，直接动现有根盘仍然不是风险最小的方案

也就是说，单纯把 StaticFlow 数据迁到 `/srv/staticflow-data` 这种发行版根盘路径，在概念上最简单，但在这台机器当前容量条件下并不合适。

### 3.3 运维边界

这次方案还受几个明确边界约束：

- 不改动已有系统盘 `D:\wsl_ubuntu\ext4.vhdx`
- 不对现有 `E:` 盘原地重格式化
- 不做双根盘或 RAID 根盘这类维护成本高的结构
- 尽量保留“数据仍在 E 盘”的运维心智

---

## 4. 方案比较与决策依据

在真正落地之前，评估过几种可行路径。

| 方案 | 优点 | 主要问题 | 结论 |
|---|---|---|---|
| 继续使用 `/mnt/e/static-flow-data` | 不迁移，最省事 | DrvFs/9p 语义不适合数据库，compaction 与 blob 读写风险持续存在 | 不采用 |
| 直接迁到发行版根盘 `/srv/...` | 真正 ext4，最原生 | 当前根盘可用空间偏紧，不适合再承载大体量数据库 | 暂不采用 |
| 直接扩现有根盘到 2TB | 根盘路径最简单 | 会直接动现有 Ubuntu 系统盘，回滚和风险隔离都不理想 | 不作为第一落点 |
| 在 `E:` 上新建 ext4 数据 VHDX | 物理数据仍在 `E:`，WSL 内部得到 ext4 语义，和系统盘隔离清晰 | 需要一次性创建、分区、格式化和挂载流程 | 最终采用 |
| 双盘 RAID 后作为根盘 | 理论上可扩展容量 | 对 WSL 根盘启动链不友好，维护复杂，收益不高 | 不采用 |

最终方案选择“**现有根盘保持不动 + 新建独立 ext4 数据 VHDX**”，原因很直接：

- 风险隔离最清楚
- 不碰已有 Ubuntu 系统盘
- 业务数据能获得 Linux 原生 ext4 行为
- Windows 侧仍然只多出一个普通的 `.vhdx` 文件

---

## 5. 最终架构与数据流

最终落地结构如下：

```mermaid
flowchart TD
    A[Windows D:\\wsl_ubuntu\\ext4.vhdx] --> B[WSL Ubuntu 根盘 /]
    C[Windows E:\\wsl-disks\\data-4tb.vhdx] --> D[wsl --mount --vhd]
    D --> E[/mnt/wsl/data4tb]
    E --> F[/mnt/wsl/data4tb/static-flow-data]
    F --> G[StaticFlow Content DB]
    F --> H[StaticFlow Comments DB]
    F --> I[StaticFlow Music DB]
    J[/mnt/e/static-flow-data] -.历史路径 / DrvFs .-> K[不再作为正式数据库主路径]
```

这个结构里有两个关键点：

1. Ubuntu 根盘和数据盘彻底分离。  
2. StaticFlow 最终只使用 `/mnt/wsl/data4tb/static-flow-data`，不再依赖 `/mnt/e/static-flow-data` 作为正式主库。

---

## 6. 根盘扩容边界与最终取舍

### 6.1 根盘扩容能力

这台机器的 WSL 版本支持：

```powershell
wsl --manage Ubuntu --resize 2TB
```

这意味着从工具能力上，直接扩现有 Ubuntu 根盘是可行的。

### 6.2 根盘扩容未作为首选的原因

虽然可行，但这次没有优先这样做，原因不是“做不到”，而是“不值得先动”：

- 现有 Ubuntu 系统盘已经稳定在用
- 当前问题的真正焦点是数据库路径在 DrvFs，而不是 WSL 本身不能提供 ext4
- 只要加一块数据盘，就已经能把数据库从 DrvFs 迁走
- 从回滚角度看，新增一个 `data-4tb.vhdx` 比改现有系统盘更安全

这也是这次所谓“扩容”的真实含义：  
**不是先改系统根盘，而是先扩展一块独立的数据能力层。**

---

## 7. 数据盘创建与 ext4 初始化流程

这部分是最终真正执行并保留下来的流程。

### 7.1 数据 VHDX 创建

先在 Windows 上准备目录：

```powershell
mkdir E:\wsl-disks
```

然后使用 `diskpart` 创建动态 VHDX：

```powershell
diskpart
```

进入 `diskpart` 后：

```text
create vdisk file="E:\wsl-disks\data-4tb.vhdx" maximum=4194304 type=expandable
exit
```

这里的要点有两个：

- `maximum=4194304` 表示 4TB 级别上限
- `type=expandable` 表示动态扩展，不会一开始就占满 4TB

### 7.2 裸盘附加

第一次把 VHDX 接入 WSL 时，先用裸盘方式：

```powershell
wsl --mount --vhd E:\wsl-disks\data-4tb.vhdx --bare
```

这个阶段只是把磁盘设备交给 WSL，还没有分区和文件系统。

### 7.3 分区与格式化

进入 WSL 后查看新磁盘设备：

```bash
lsblk -o NAME,SIZE,TYPE,MOUNTPOINTS
```

假设新盘是 `/dev/sde`，再执行：

```bash
sudo parted /dev/sde --script mklabel gpt mkpart primary ext4 1MiB 100%
sudo mkfs.ext4 -L data4tb /dev/sde1
```

这样就完成了：

- GPT 分区表创建
- 单分区建立
- ext4 文件系统格式化

### 7.4 正式挂载

格式化完成后，先卸载裸盘，再按分区方式正式挂载：

```powershell
wsl --unmount E:\wsl-disks\data-4tb.vhdx
wsl --mount --vhd E:\wsl-disks\data-4tb.vhdx --partition 1 --type ext4 --name data4tb
```

此时 WSL 内部会出现：

- `/mnt/wsl/data4tb`

可以这样验证：

```bash
findmnt /mnt/wsl/data4tb
df -h /mnt/wsl/data4tb
```

---

## 8. 启动自动化尝试与失败原因

这次真正踩坑的，不是挂盘本身，而是“想把这件事自动化”时踩到的启动顺序问题。

### 8.1 `fstab` 方案的失败机制

最初为了让这块盘固定映射到 `/srv/data4tb`，尝试过在 `/etc/fstab` 中加入：

```fstab
/mnt/wsl/data4tb /srv/data4tb none bind 0 0
```

但这会导致 WSL 每次启动时出现：

```text
wsl: Processing /etc/fstab with mount -a failed.
```

根因在于启动顺序：

1. WSL 先启动发行版。
2. 启动过程中先处理 `/etc/fstab`。
3. 这时 `data-4tb.vhdx` 还没有通过 `wsl --mount --vhd ...` 接入。
4. `/mnt/wsl/data4tb` 不存在或尚未就绪。
5. bind mount 失败。

因此，**额外挂载的数据盘不适合依赖发行版启动早期的 `fstab` 去消费**。

### 8.2 计划任务方案的失效原因

也尝试过让 Windows 计划任务在登录时自动执行 `wsl --mount --vhd ...`。  
从理论上说，这个方向可行，但实际体验并不理想，原因包括：

- 任务运行上下文不透明，排查成本高
- 即使计划任务触发，也不如直接手动执行那条管理员 PowerShell 命令稳定
- 一旦中间再叠一层“先启动发行版、再执行脚本、再尝试 bind mount”的包装，失效面会扩大

最后的经验很简单：  
**对这种需要管理员权限的挂盘动作，最可靠的不是复杂自动化，而是一个尽量薄的调用包装。**

### 8.3 最终保留的启动方式

最终保留下来的只有一条核心命令：

```powershell
wsl --mount --vhd E:\wsl-disks\data-4tb.vhdx --partition 1 --type ext4 --name data4tb
```

后续做的桌面脚本，也只是把这条命令做成带管理员提权的双击启动器，而不是再引入额外逻辑。

这个取舍的理由是：

- 失败面最小
- 排查路径最短
- 行为与手工验证完全一致

---

## 9. 权限模型与业务目录划分

### 9.1 挂载根目录的默认权限

挂载完成后，`/mnt/wsl/data4tb` 默认是典型的 ext4 根目录权限：

```text
drwxr-xr-x root root
```

这意味着普通用户 `ts_user` 可以读取和进入目录，但没有写权限。

### 9.2 整盘根目录不改属主的原因

可以直接把整个 `/mnt/wsl/data4tb` 改给 `ts_user`，但这不是最好的边界划分。  
更干净的做法是：

- 挂载根目录仍然保持 `root` 管理
- 在盘内创建专门的业务目录
- 只把业务目录赋予 `ts_user`

最终采用的是：

```bash
sudo mkdir -p /mnt/wsl/data4tb/static-flow-data
sudo chown -R ts_user:ts_user /mnt/wsl/data4tb/static-flow-data
```

这样带来两个好处：

1. 根目录保持“系统资源”语义。  
2. 业务数据目录拥有稳定写权限，后续服务进程和手工运维都更自然。

### 9.3 权限持久化特征

这里的 `chown` 是对 ext4 文件系统元数据的真实修改，不是 DrvFs 的映射行为。  
因此：

- 重新进入 WSL 后仍然保留
- 重新挂载数据盘后仍然保留
- 不需要每次重复设置权限

---

## 10. StaticFlow 的最终落地路径

最终正式使用的业务路径是：

```text
/mnt/wsl/data4tb/static-flow-data
```

对 StaticFlow 来说，可以直接把根目录配置成：

```bash
DB_ROOT=/mnt/wsl/data4tb/static-flow-data
```

如果采用显式三库路径，也可以写成：

```bash
DB_PATH=/mnt/wsl/data4tb/static-flow-data/lancedb
COMMENTS_DB_PATH=/mnt/wsl/data4tb/static-flow-data/lancedb-comments
MUSIC_DB_PATH=/mnt/wsl/data4tb/static-flow-data/lancedb-music
```

旧数据从 `/mnt/e/static-flow-data` 迁移过来的方式也很直接：

```bash
rsync -a --info=progress2 /mnt/e/static-flow-data/ /mnt/wsl/data4tb/static-flow-data/
```

迁移完成后，StaticFlow 的正式数据库读写就不再走 DrvFs，而是走这块独立 ext4 数据盘。

---

## 11. 运行时行为变化

这套方案上线后的核心变化，不是“Windows 盘符变了”，而是数据库运行语义发生了变化。

### 11.1 文件系统语义变化

从 `/mnt/e` 切到 `/mnt/wsl/data4tb` 之后，数据库获得的是：

- 原生 ext4 inode / metadata 行为
- 更符合 LanceDB / object_store / Rust IO 栈预期的 rename 和目录语义
- 不再依赖 Windows NTFS 到 WSL 的翻译层

### 11.2 业务观测变化

对 StaticFlow 来说，最重要的收益主要集中在以下工作负载：

- compaction
- prune
- blob 读取
- manifest / fragment 维护
- 后台维护与前台读取并发

这些负载不再和 `/mnt/e` 的 DrvFs 层语义绑定在一起，出现“像卡死一样”的概率会显著下降。

---

## 12. 运维操作手册

### 12.1 数据盘接入流程

每次需要把数据盘接入 WSL 时，核心命令仍然只有这一条：

```powershell
wsl --mount --vhd E:\wsl-disks\data-4tb.vhdx --partition 1 --type ext4 --name data4tb
```

验证方式：

```bash
findmnt /mnt/wsl/data4tb
df -h /mnt/wsl/data4tb
```

### 12.2 迁移流程

停止后端后执行：

```bash
rsync -a --info=progress2 /mnt/e/static-flow-data/ /mnt/wsl/data4tb/static-flow-data/
```

迁移后检查：

```bash
ls /mnt/wsl/data4tb/static-flow-data
```

再启动服务并做实际读写验证。

### 12.3 目录写权限检查

验证用户目录是否可写：

```bash
touch /mnt/wsl/data4tb/static-flow-data/.write-test
rm /mnt/wsl/data4tb/static-flow-data/.write-test
```

如果失败，通常说明属主或权限没有正确设置。

---

## 13. 故障场景与恢复路径

### 13.1 `fstab` 启动报错场景

症状：

```text
wsl: Processing /etc/fstab with mount -a failed.
```

判定：

- `fstab` 里依赖了 `/mnt/wsl/data4tb`
- 但 VHDX 尚未 attach

处理：

1. 删除相关 `fstab` bind mount 项。
2. 不再让发行版启动阶段负责消费该挂载点。
3. 保留显式的 `wsl --mount --vhd ...` 入口。

### 13.2 目录无写权限场景

症状：

- `ts_user` 可以 `cd /mnt/wsl/data4tb`
- 但无法在该路径下创建文件

判定：

- ext4 根目录仍为 `root:root` 且 `755`

处理：

```bash
sudo mkdir -p /mnt/wsl/data4tb/static-flow-data
sudo chown -R ts_user:ts_user /mnt/wsl/data4tb/static-flow-data
```

### 13.3 数据盘未挂载场景

症状：

- `/mnt/wsl/data4tb` 不存在
- 或者业务目录缺失

处理：

在管理员 PowerShell 里重新执行：

```powershell
wsl --mount --vhd E:\wsl-disks\data-4tb.vhdx --partition 1 --type ext4 --name data4tb
```

再回 WSL 验证挂载点。

### 13.4 路径误指向 DrvFs 场景

症状：

- 服务仍然读取 `/mnt/e/static-flow-data`
- compaction 期间继续出现高延迟

处理：

检查启动环境变量，确保使用的是：

```bash
DB_ROOT=/mnt/wsl/data4tb/static-flow-data
```

而不是历史路径 `/mnt/e/static-flow-data`。

---

## 14. 方案边界与后续演进

这套方案解决的是“数据库正式路径的文件系统语义”问题，但它并不意味着：

- Ubuntu 根盘从此不需要扩容
- 所有 Windows 与 WSL 之间的数据共享问题都被消除
- 所有后台 IO 卡顿都必然完全消失

它的边界很明确：

- 它优先解决 LanceDB 主库不再跑在 DrvFs 上的问题
- 它优先保证数据库路径获得 ext4 语义
- 它不替代未来对根盘容量的长期治理

如果后续业务继续增长，下一步可以再单独评估：

- 是否扩现有 Ubuntu 根盘到 2TB
- 是否再新增第二块数据 VHDX
- 是否为不同业务库拆分独立数据盘

---

## 15. 命令索引

### 15.1 Windows 管理员 PowerShell

```powershell
mkdir E:\wsl-disks
wsl --mount --vhd E:\wsl-disks\data-4tb.vhdx --bare
wsl --unmount E:\wsl-disks\data-4tb.vhdx
wsl --mount --vhd E:\wsl-disks\data-4tb.vhdx --partition 1 --type ext4 --name data4tb
```

### 15.2 Windows `diskpart`

```text
create vdisk file="E:\wsl-disks\data-4tb.vhdx" maximum=4194304 type=expandable
exit
```

### 15.3 WSL 分区与格式化

```bash
lsblk -o NAME,SIZE,TYPE,MOUNTPOINTS
sudo parted /dev/sde --script mklabel gpt mkpart primary ext4 1MiB 100%
sudo mkfs.ext4 -L data4tb /dev/sde1
```

### 15.4 WSL 目录与权限

```bash
sudo mkdir -p /mnt/wsl/data4tb/static-flow-data
sudo chown -R ts_user:ts_user /mnt/wsl/data4tb/static-flow-data
```

### 15.5 StaticFlow 数据迁移

```bash
rsync -a --info=progress2 /mnt/e/static-flow-data/ /mnt/wsl/data4tb/static-flow-data/
```

---

## 16. 总结

这次存储改造的关键，不是“把数据库换个目录”这么简单，而是把 StaticFlow 从一个对数据库不友好的 DrvFs 路径，迁到了一个真正具备 ext4 语义的数据盘上。

最后保留下来的方案其实非常克制：

- 不动现有 Ubuntu 系统盘
- 不搞双根盘或 RAID 根盘
- 不依赖脆弱的 `fstab` 启动顺序
- 只新增一个 `E:\wsl-disks\data-4tb.vhdx`
- 只依赖一条明确的 `wsl --mount --vhd ...` 接入命令
- 业务最终使用 `/mnt/wsl/data4tb/static-flow-data`

这套结构的工程价值在于，它把“Windows 盘上存储数据”和“WSL 内部得到 ext4 语义”这两件事同时满足了，并且把复杂度控制在了一个很容易解释、很容易排障的范围内。
