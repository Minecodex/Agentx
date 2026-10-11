# plan5：Workflow Studio 与节点体系重构

更新日期：2026-09-02。目标是按 [demo2.html](demo2.html) 重建节点配置体验，并让画布上的配置能够保存、编译和真实执行。plan5.2进一步把所有可绑定字段统一为自然输入和递归InputBinding，删除公共输出投影，重做Code输出示例、TCP出站、Loop/Merge与可拖拽Inspector；实施任务、文件落点和门禁见 [implementation-plan.md](implementation-plan.md)。

架构判断、A1–A7局部优化与实测门禁见 [architecture-review.md](architecture-review.md)。总体分层继续使用，契约来源、状态模型和边界实现已完成局部收口。

> **当前状态：plan5.2已完成。** 契约、Runtime、Gateway、共享输入、13类专用面板、Loop/Merge画布和Inspector均已破坏性切换；本地环境已清空并按当前DDL重建。最终Kubernetes运行ID为`8f06dd0c07`：20项全部通过，临时namespace已purge，开发服务已恢复为1副本。

## 0. 兼容与实施边界（硬约束）

**本项目处于开发阶段：不考虑历史数据兼容，也不考虑旧代码、旧接口和旧协议兼容。** 此要求适用于 plan5 的全部前端、后端、数据库、Schema、测试和文档修改。

1. 旧 Workflow Definition、草稿、Editor Document、调试数据、版本快照、审批记录和测试 Fixture 不需要迁往新结构；受本次重构影响的数据按新契约重新创建，不为保留旧数据扭曲新设计。
2. 不编写数据迁移脚本，不新增逐版本升级 migration；涉及表、列和状态变化时直接修改所属初始化/基线 DDL，并在实施阶段重建相应开发或测试数据。`migrations/` 目录作为现有数据库初始化基础设施保留，不等于继续维护历史升级链。
3. 旧节点、旧参数、旧枚举、旧路由、旧函数入口和旧序列化分支直接删除。不得保留兼容层、shim、字段自动转换、旧 JSON 双读、别名、deprecated 包装、兼容性 fallback 或新旧双入口。
4. 不将旧 `approve/reject` 决策接口包装到新决策接口，不把旧附件节点折叠为新槽位，不把旧循环控制边转换为新容器关系，也不并行维护两套子流程输入配置。调用方、生成物、Fixture 和文档一起切换。
5. 不兼容历史不等于删除仍有职责的能力：资源授权、凭据保护、Provider、Agent 附件执行、幂等、重试、取消、故障恢复和运行预算继续保留。业务默认值和错误处理不是旧代码兼容层。
6. 遵守根 `AGENTS.md`：保留用户已有的无关工作树修改；只做满足本需求的改动。删除对象、重建环境和受影响数据必须有明确范围，不能借“不兼容”清理无关资源。
7. 实施已按上述边界完成；开发与测试环境使用当前初始化DDL重建，没有增加历史数据迁移或兼容入口。

## 1. 最终交付状态

以下是 2026-09-02 对当前工作树与临时Kubernetes环境的最终核对结果。

| 范围 | 已完成内容 | 验收证据 |
|---|---|---|
| 契约与Catalog | Definition 8.0、Registry权威Catalog、不可变Manifest/hash、动态端口、Exit顺序及生成物一次性切换 | 契约、Schema和Catalog漂移测试通过；旧版本与未知字段明确拒绝 |
| 破坏性清理 | 删除旧数据节点、switch、wait、error_handler、remote_action、binding边、旧Schema/OpenAPI与调用路径 | boundary-check通过；搜索与拒绝旧输入测试通过 |
| Studio | 聚焦路由、240/264px画布布局、480/440px可拖拽Inspector、六组分类色、13类专用面板、变量Picker、F8调试、Pin/Mock和编辑历史 | 前端84个测试文件、371项测试通过；纯UI创建并运行Start→IF→Set→Merge→Exit，13类面板配置后保存重开、黄金截图与字段错误通过 |
| 数组与控制 | List固定数组契约；Loop隐式入口、并行/恢复/顺序/三种错误模式；IF与Approval稳定动态端口 | Rust状态机、Runtime集成和Kubernetes continue/remove真实失败轮次、顺序及Workflow Runtime滚动重启恢复通过 |
| Approval与Exit | 统一Decide、按钮快照、固定生命周期、终态清理；first_return/all_complete顺序与null对齐 | 第三按钮、认领/决定、挂起清理和多Exit E2E通过 |
| 集成与AI | Agent六槽、HTTP认证/脱敏、Sub-workflow inputs、Model JSON Schema、Code命名输入与结构化输出 | Agent附件、会话/压缩/Memory、Bearer/Basic/API Key/custom_json、二进制Artifact、子流程、Model，以及Python/JavaScript/Shell通过Egress Gateway访问白名单HTTP和原始TCP并拒绝受保护目标的真实运行通过 |
| 架构边界 | Provider受控出口、Schema所有权清单与生产文件拆分全部收口 | `agentx-boundary-check`通过；生产前后端文件均不超过2000行 |
| 系统验收 | 临时namespace内完成产品、性能、恢复、升级、隔离和清理 | `pytest tests/e2e`：20 passed in 1038.06s，运行ID`8f06dd0c07`；临时namespace已删除且开发服务恢复 |

本地门禁同时通过`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`、前端构建与84文件/371项全量测试、TypeScript、Ruff、16项acceptance、生成物漂移和boundary-check。Cargo测试中两个需要真实钉钉/飞书凭据的外部探针保持ignored，不作为平台连通性证据。

## 2. 决策记录

### 2.1 已确认约束

| 编号 | 决策 |
|---|---|
| D1 | remote_action 整条协议废弃；内部流程调用使用 sub_workflow。保留仍使用的 Provider 与 MCP 资源执行能力 |
| D2 | Loop 复用现有 generation/runIndex 状态机；不为每个元素创建独立 Execution，不新增循环服务 |
| D3 | Definition 保持扁平节点数组，`parentId` 仅指向 loop_over_items；容器不可嵌套 |
| D4 | wait 节点删除，挂起型人工交互由 approval 承载；审批按钮与分支同源 |
| D5 | 13类节点使用定制业务面板；共享字段控件，不保留通用参数自动渲染主路径 |
| D6 | `end.outputs` / `end.error.outputs` 是全局共享契约；各 exit 只保存自己的值绑定 |
| D7 | `first_return` 默认；`all_complete` 按 Exit 定义顺序将每个输出字段汇总为数组 |
| D8 | error 端口常驻；接线即失败分支。删除节点级 onError 和 End 错误收集策略 |
| D9 | Inspector在1440视口默认480px、1280视口默认440px，可拖拽420–640px并保存本地偏好；节点栏按空间自动折叠 |
| D10 | 输入与 contexts 在 Start；输出与完成模式在 Exit；WorkflowSettings 保留在工作流工具栏 |
| D11 | 不兼容历史数据或旧代码；按第0节直接清理、切换和重建，不留兼容层或迁移链 |
| D12 | 参照Dify的是节点创建、专用配置、变量选择、单节点调试和结果查看流程；不复制Dify内部协议、节点总数或存储模型 |
| D13 | Picker只展示目标节点经execution边可达的上游；后端Draft保存执行同源的轻量引用可达性校验，非法引用不得增加revision |
| D14 | `all_complete`只聚合实际到达的Exit并按Definition顺序排列；每个输出字段为每个到达Exit保留槽位，可选值缺失写null，必填值缺失仍失败 |

### 2.2 M0 已冻结事项

以下事项已冻结并由当前实现、契约与测试共同约束。

| 事项 | 最终决策 |
|---|---|
| 编辑器页面空间 | 仅editor路由使用聚焦布局，隐藏企业侧栏和全局顶栏；普通企业页面不改 |
| 品牌主色 | 保留全站紫色主色；节点分类使用六组语义色 |
| 分类色 | Merge归逻辑青，List归转换蓝；卡片、Palette和Inspector同源 |
| 数组结果 | List和Loop均输出一个ExactlyOne Item：`{"items":[...]}`，无结果别名 |
| 默认值 | Loop `parallelism=1`、`errorMode=terminate`；Exit `first_return`；timeout只来自NodeSettings |
| Code 调用方式 | Python `main(**inputs)`；JavaScript `main({name,count})`；Shell使用固定JSON输入/输出文件；结果按显式Schema校验 |

## 3. 视觉与交互基线

### 3.1 共用规格

| 对象 | 目标 |
|---|---|
| 节点 / 节点栏 / Inspector | 节点240px、节点栏264px；Inspector默认480/440px且可拖拽420–640px；节点栏可收起 |
| Studio 输入与选择控件 | 统一紧凑规格：30px高、12px字；企业表单36px默认规格保留 |
| 卡片标题 / 图标 | 标题13px；画布图标24px；Inspector图标28px，沿用节点分类色与图标 |
| 分类 | 起始 `#155EEF`、AI `#6366F1`、逻辑 `#06B6D4`、转换 `#3B82F6`、集成 `#8B5CF6`、输出 `#F59E0B` |
| 模式选择 | 单列满宽，统一选择标记、内边距、圆角、说明层级；不按内容长度产生不同宽度 |
| 字段与操作 | 统一字段行、变量胶囊、条件行、KV行、添加按钮和Toggle；避免层层通用JSON框 |
| Inspector | Start/Exit/普通节点共用头部规格；基本属性和高级配置次要化；输出契约仅显示一份 |

保留输入/输出/Trace、Pin/Mock、执行轨道、保存、版本、发布及授权反馈；demo未画出这些能力，不构成删除理由。只调整其布局、密度和展开方式。浅深主题使用同一套语义令牌；正式范围仅为桌面端。

### 3.2 变量与配置交互

- 普通字段展示“节点显示名 · 字段”胶囊；默认按可达上游节点分组，直接展示业务字段，不先展示空白命名空间面板。
- 原生 Selector 继续保存稳定 Node ID、端口、run/item选择和结构化路径；名称变化只改变显示，不重写引用。
- `current/first/all/runs`、端口基数等技术信息按需展开；类型过滤、敏感字段限制和明确错误提示保留。
- Loop 子节点的变量目录必须包含作用域内的 `loop.item/loop.items/loop.index`，分别表示当前元素、完整循环数组和序号；IF/Approval动态端口与子流程输出类型必须按实例/目标版本生成。
- 普通字段统一保存递归`InputBinding`的`literal/reference/template/array/object`；IF与List过滤保存左右值同为InputBinding的ConditionSpec。JSON5只用于Studio编辑，递归表达式AST不进入Definition、Manifest或前端类型，复杂计算由Code承担。
- 没有明确目标节点时输出目录为空；Exit成功/错误映射使用各自的虚拟目标计算可达前驱，不能复用当前选中节点。
- 已保存引用在断线后保留原值并显示字段级错误；保存被拒绝且revision不变，禁止静默删除或改写引用来得到绿灯。

### 3.3 节点操作闭环

正式交互按同一条可验收链路组织：从节点栏、端口快捷入口或边上插入创建节点并自动接线；选中节点打开专用Inspector；从可达上游选择变量；保存并重开；运行当前节点；在同一Inspector查看Input、Output和Trace；使用Pin/Mock调整调试输入后再次运行；修改动态分支后同步卡片摘要、Handle、连线和引用；删除与撤销恢复为一个编辑事务。

现有单节点运行、Input/Output/Trace、Pin/Mock和执行轨道继续复用。面板重构不得只保留配置表单而丢失调试闭环，也不另建第二套调试数据协议。

## 4. 目标13类节点与责任分工

| 分组 | 类型 | 前端目标 | 后端范围 |
|---|---|---|---|
| 起始 | start | 名称/类型/必填同行；复杂Schema再展开；contexts与系统提示 | 复用输入和Context契约 |
| AI | model | 模型摘要、系统/用户提示词、变量混排、结构化输出配置 | 接通结构化请求与校验；资源摘要仅暴露安全元数据 |
| AI | agent | 六槽真实管理、计数、添加/更换/移除、授权状态与卡片徽标 | 复用bindingSlots/resourceReferences和授权，不重构资源模型 |
| 逻辑 | if | IF/ELIF/ELSE区块；变量/操作符/右值同行；AND/OR | 沿用AST，统一稳定分支ID、类型校验和动态端口 |
| 逻辑 | loop_over_items | 空节点即容器；数组输入、每轮输出、并行数、失败模式 | 数组求值、隐式入口、轮次上限、输出聚合、恢复与预算 |
| 逻辑 | merge | 三种满宽模式卡；相关Join字段按需展开；业务端口名 | 复用append/combine_by_position/combine_by_key |
| 逻辑 | approval | 标题/说明、用户选择、按钮名称、超时开关和时间单位 | 任务/决策分离，快照投影到详情，统一决策恢复 |
| 转换 | code | 语言、网络策略、命名输入mapper、Monaco、明确输出字段 | 在已有OpenSandbox路径接通输入与结果契约 |
| 转换 | set | 字段名+固定值/变量同行，统一保留字段开关 | 复用values/keepOnlySet |
| 转换 | list | 数组输入、条件行、排序字段/方向/空值策略、取前N | 对所选数组逐元素过滤、排序、截断 |
| 集成 | declarative_http | method+单层URL；参数/头/体页签；认证与超时 | 查询参数编码、Vault凭据注入、真实传输deadline |
| 集成 | sub_workflow | 已发布版本、打开子流程、输入mapper与输出契约 | 映射求值→校验→子执行，复用Composite生命周期 |
| 输出 | exit | 共享字段与本出口值同行对应；紧凑成功/错误区；单列完成模式 | 定义顺序、数组类型传播、终态清理 |

设计器目录仅暴露上述集合；Start与Exit沿用各自的边界/终止节点处理。`mcp_tool/skill/rag/memory` 不再作为独立画布节点入口，但其资源和执行能力继续服务于Agent附件。

## 5. 运行与数据契约

### 5.1 Definition、Manifest和生成物

当前目标仍为不兼容旧版本的 Definition 8.0。更新domain、IR、Node Manifest、运行DTO、生成Schema/OpenAPI、前端类型和Fixture时一次性切换，不新增双读。

Manifest负责节点身份、六组分类、显示名、图标key、字段/枚举文案、端口、槽位和能力；尺寸、颜色令牌与具体布局归前端。未知业务字段不能统一标成“参数”，也不能让前端另猜一份目录。不增加样式配置服务。

Definition与NodeManifestVersion统一由 `agentx-runtime-contracts` 的 `generate-contracts` 生成到 `contracts/schemas/runtime-v1/`，继续使用漂移测试。旧根目录Schema和remote_action OpenAPI不恢复。

### 5.2 IF与动态端口

`cases[]` 中每项保存稳定 `id`、显示名、`conditions[]` 和 `logicalOp`。条件按序首个命中；输出handle为 `case:{id}`，另有 `else/error`。编译器、Worker、连线、引用目录和结果契约使用同一套展开端口，不接受未声明ID或旧switch映射。

### 5.3 List与数组边界

`parameters.input` 必须实际求值为数组；`filter.conditions/logicalOp`、`sort[]`、`takeN` 作用于该数组的元素。复用现有 filter→sort→takeN 算法，但每个元素的条件上下文不能仍绑定到外层Item。

底层Item/Port模型不变。HTTP只发布固定顶层`statusCode/headers/body/files`，其中`body`为字符串；需要JSON子字段时先由Code解析并声明结构化输出Schema，再交给List或Loop。数组结果字段与Schema用于前端选择器、编译和运行输出校验。

### 5.4 Loop容器

- 节点保持扁平数组，子节点以 `parentId` 归属容器；只允许Loop为父，禁止嵌套、包含环和体内DAG环。
- 体内入口由容器内入度为0的节点推导。Definition不保留“容器→子节点”的手工控制边；跨容器边界的真实连线直接报错。
- `${loopId}::iteration-start`、`${loopId}::iteration-end` 及其起止连线是纯编辑视图，不进入Definition，不通过序列化改写为真实Loop边；入口连接体内入度为0节点，出口按main/error分别连接体内出度为0节点。容器外部输入、输出、错误端口与普通节点复用同一长条Handle、标签和快捷新增交互。
- 参数为数组 `input`、每轮 `outputSelector`、`parallelism`、`errorMode`。并行数必须限制活跃元素轮次，不能只显示在卡片或一次性创建全部轮次。
- 每轮建立 `loop.item/loop.items/loop.index`；`outputSelector` 的 Picker 以编辑态迭代结束节点为目标，允许选择循环体末端链路的完整对象或属性，并按输入顺序汇总。不把所有末端节点对象隐式混合来替代用户选择的结果。
- `terminate` 终止循环并按error接线处理；`continue` 为失败项产出null；`remove` 剔除失败项。保留activationBudget、取消和恢复；检查点必须能恢复未完成轮次与聚合状态。
- 前端覆盖首次加入子节点、chip可见、父相对坐标、resize/自动扩容、拖入拖出、复制粘贴、撤销重做、保存重开和递归自动布局。

### 5.5 审批：生命周期与决策分离

按钮仍为 `buttons: [{id, label}]`；默认通过/拒绝是按钮数据。标题、说明和用户选择使用实际参数；`timeoutMs`由前端数字/单位控件生成，不能并行维护多份互相冲突的超时值。

任务生命周期固定为待处理、已认领、已决策、超时、取消等状态；业务按钮ID单独作为 `decision` 保存，不能写成任意task.status。具体DTO在M0冻结，所有业务决策统一走Decide，不保留旧approve/reject接口包装。

执行闭环：

1. 挂起时写任务和不可变buttons快照。
2. Runtime事件/全量快照→Control投影→列表/详情API均传同一份按钮数据。
3. 审批人认领后按按钮快照展示动作，提交稳定decision ID和reason。
4. 校验任务版本、候选人、认领状态、按钮归属与幂等；原子写receipt/actor/reason和恢复命令。
5. 恢复端口为 `decision:{id}`，超时为 `timed_out`；恢复确认、筛选、徽标和历史使用新生命周期。
6. 执行提前终态时让未完成审批失效并导出事件，禁止后续点击复活执行。

只修改Workflow运行审批，不波及资源授权审批。当前先接通已有“指定用户”能力；“按组织自动解析部门主管”是独立业务规则，不作为静态控件伪装实现。

### 5.6 完成模式与错误

- `end.outputs/end.error.outputs` 统一声明输出；各Exit的 `parameters.outputs/errorOutputs` 只保存取值。共享字段的改名、改型与删除必须校验所有Exit绑定。
- `first_return`：首个main/error交付决定成功/失败，并取消剩余激活、清理挂起任务；不回滚已经发生的HTTP等副作用。
- `all_complete`：等待全部可完成分支；未处理失败或错误交付按统一错误规则终态。IR显式保存Exit定义顺序，成功结果只包含实际到达的Exit并按该顺序聚合，不能依赖BTreeMap的ID排序。
- 每个成功输出字段包装为数组，并为每个实际到达的Exit保留一个位置；未绑定或求值为omit的可选字段写null，对应数组元素Schema允许null。必填字段缺失/null仍失败，敏感约束继续有效。
- 编译器、子流程Manifest、发布/API输出Schema、父画布变量目录和实际返回值必须同源。
- 节点有error出边才路由错误输出；无旧onError策略、无collectWindowMs、无旧结果格式兜底。

### 5.7 HTTP与子流程

HTTP保留method/url/query/headers/body和认证契约。URL、query、header及JSON请求体统一使用InputBinding；文字与变量可以在同一输入框拼接，数组/对象用JSON5编辑并保存结构化Binding。响应body按Dify式固定为字符串，二进制写Artifact/files。认证引用已有Vault凭据与授权，Secret不进入草稿、浏览器结果或日志；timeout统一使用节点settings/deadline并传到真实transport。

子流程只有一个产品入口。选择不可变版本后获取其输入/输出/Context契约；显式inputs映射必须在父上下文求值并校验，实际传给子执行。已有Composite的快照、父子关系、超时取消、回传和Context提交继续复用，删除旧的未映射调用路径及双套配置。

### 5.8 Model与Code

2026-10-11 保存与资源申请修订：自动保存只确认本次发送的持久化快照，保存期间继续编辑时仍保留未保存状态。资源申请与已有保存串行，并绑定最新已保存修订；未完成的 Agent 如果需要授权才能保存，使用既有 Workflow 主体/资源包申请范围，不把更早的草稿修订声明为来源。后台资源包、请求者与已声明来源修订的失效校验保留。

模型结构化输出需要参数Schema、受支持的Provider原生请求配置、冻结输出契约与结果校验；不解析普通文本兜底，不支持此能力的模型给出明确配置错误。

Code复用Monaco与OpenSandbox，命名输入使用InputBinding映射。用户填写JSON5对象`outputExample`，Compiler确定性推导冻结Schema，Runtime只接受对象根结果并校验；下游Picker直接展示业务字段和完整对象，stdout/stderr/exitCode/files归诊断分组。网络策略使用显式目标与端口段，Sandbox只连接Egress Gateway，HTTP与任意TCP分别使用标准代理变量和HTTP CONNECT，不增加Agentx SDK。

## 6. 删除范围与保留边界

- 删除旧switch、error_handler、wait、no_op、stop_and_error、remote_action节点/协议入口。
- 删除旧filter/sort/limit类型，由list承载；删除aggregate、remove_duplicates、split_out、rename_fields、json_transform、date_time、base64、hash、compare_datasets、structured_validator、item_generator等旧独立节点。
- 删除旧attachment节点、binding虚线边和创建入口、旧接口面板、通用Inspector主路径、旧循环控制边路径、固定双分支审批决策入口及过期生成物。
- 清理对应调用方、枚举、默认值、测试与文档；不能只在节点栏隐藏但保留一套旧代码可继续调用。
- 保留仍有真实用途的资源执行器、Node Manifest/Provider、echo-node Provider Fixture、审批挂起机制、共享Trace Wait语义、权限、存储和运行可靠性机制。
- 不更换React Flow、Radix/Tailwind、Lexical、Monaco、ELK、Zustand、TanStack Query；不另造表达式协议、UI配置服务或运行服务；不做移动端。
- Definition、Editor Document、Debug Overlay、运行数据四者保持分离；Control/Runtime/Observability的数据访问边界不变。

## 7. 实施与验收结果

M0–M6、F1–F8和plan5.1保留为历史基线。plan5.2按 [实施计划](implementation-plan.md) 重新打开V0–V5门禁；只有新契约、自然输入、13类面板、Loop/Merge、Gateway TCP、黄金截图、本地全量测试和Kubernetes E2E全部通过并完成环境清空重建后才关闭。

最终验收满足“按专用交互配置，并由真实后端执行产生对应结果”：

- Studio通过节点栏、动态端口快捷添加和边上插入创建/接线，支持保存重开、字段错误、单节点运行、Input/Output/Trace、Pin/Mock及撤销删除。
- 13类节点覆盖空态、已配置态和关键错误态；桌面浅深主题、中英文及1280/1440/1920视口通过产品Playwright。
- 500/1000节点性能spec分别达到median 59.9/59.9 FPS、p95输入延迟42.3/37.8ms，通过既定门槛。
- Kubernetes覆盖纯UI最小链路、数组循环三种错误模式与滚动重启恢复、自定义审批、多Exit、Agent附件、四类HTTP凭据与二进制Artifact、子流程、Model结构化输出、三种Code运行器、会话压缩、隔离、备份及升级回滚。
- 最终 `pytest tests/e2e` 结果为20 passed（1038.06s，运行ID `8f06dd0c07`）；其中产品闭环包含三种Code运行器通过Egress Gateway访问白名单HTTP和原始TCP，以及未列目标和永久阻断目标的拒绝验证；清理后无临时namespace，开发服务副本全部恢复。

该完成结论只覆盖plan5的Web桌面产品和本地Kubernetes功能门禁，不扩展为生产容量、高可用、灾备、强隔离或V2-08B认证。
