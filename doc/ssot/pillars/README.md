# README

## Intro

目前市面上已经有各种client应用实现了对于ACP协议的内部实现, 但是这些实现都是opinionated, 都是为了应用本身的特别用处而开发的 - 而没有一个不假设client/agent, 通用泛用, 对于ACP各个功能调用实现完整支持, 且专门用作ACP的注册使用+对话管理+消息收发处理的这样一个独立的应用设施 - 我称之为ACP Hub.

## short

ACP Hub是一个acp管理和调用core, 让使用者可以选择其中注册的任意ACPAgent进行对话管理和消息收发, 并尝试自动捕获维护能够获取到的消息和对话历史记录.

## Spec

通过Hub, 使用者可以:

1. 像注册MCP一样, 配置注册自己喜欢的ACP Agent Endpoint, 也可以自己直接上手, 为某个agent client开发一个ACP adapter程序并注册.
2. 全局关键字来搜索对话和消息记录, 指定某个endpoint来增/删对话, 查看对话中的消息.
3. 指定某个endpoint的某个对话, 发送消息, 等待回复, 并查看回复.
4. 设置发送消息的具体参数, 模型/思考强度/模式等等, 来支持外部文本slash command覆盖不到的, 全部的消息模式和状态.
5. 通过acp proxies来预处理发送的消息, 在其中添加工具相关主动信息, 或者对文本polish等等; 或对于收到的消息进行后处理, 将其转化为格式化信息等等.

## design

具体来说, ACP Hub可以:

1. 像注册MCP Endpoints一样, 注册stdio JSON-RPC/HTTP/WebSocket协议的 ACP Agent Endpoints
2. 不关注各个ACP Agents 的具体实现, 而是内部提供统一的 AgentEndpoint 抽象层, 并类似MCP在运行时进行capability negotiation, 以检查ACP Agent的最小必要能力和可选能力覆盖程度. 最后, 具体能够执行的各个ACP操作, 取决于ACP Agent Endpoints的能力支持程度.
3. 通过统一的ACP Endpoints抽象层, 使用stdio JSON-RPC/HTTP/WebSocket协议与注册的各个ACP Agent进程进行管理和交互.
4. 作为ACP协议中的client和conductor: Client(Hub) - ACP Conductor(Hub)&Proxies(If any) - ACP Agents
5. 本体是一个on-demand singleton daemon, 各种启动入口, cli, mcp, 内嵌库等等, 都通过文件来发现和锁定启动这个服务器, 使用rust interprocess + JSON-RPC方式和core daemon进行连接交互. 服务器在无人使用一段时间后自动退出.

## FAQ

- Conversation, 消息等静态资源, 具体指的是acp agent自己提供的对话, 还是hub自己捕获的投影?
- 没办法保证所有agent完整支持conversation的各种API, 但是一旦功能调用成功, 那么hub就会全量记录相关的静态资源snapshot, 每次调用更新, 尽可能完整体现acp agent endpoints的静态资源情况.

- acp-hub能够显示和操作的对话, 只包括通过acp-hub创建的对话, 还是endpoints能够查找到的所有对话?
- acp-hub必然是要能够CRUD当前已经存在的对话的, 而不只是自己创建的对话. acp对话记录, 和原始对话记录, 是完全平行的两层数据, 而不是同层级的互斥数据;
  - 只有在acp对应的agent endpoints完全不支持查看session内容, 历史session list的时候, 才仅fallback到我们自己的捕获记录;
  - 在大多数情况下, acp agent endpoints能够支持显式原始session, 查找session, list session, 我们的捕获数据和session的原始数据都应当被显示出来, 作为两个互相独立的信息;
  - 捕获的数据和原始对话记录的语义本来就是不相同的, 并且各自有用. 而不是只能显示我们自己创建的对话的捕获数据.
