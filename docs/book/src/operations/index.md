# 運用

エンジン本体はUSI executableまでを担当する。
CSA sessionとclockは`rsshogi-csa`、local match、SPSA、SPRT、dashboardはShogiArenaの責務である。

まず固定nodeのsmoke testでprotocolと合法手を確認し、次にSPSAで候補を生成する。
採用判断は、学習に使っていないholdout openingと独立run directoryを使うSPRTで行う。

外部CSAサーバへ接続する前には、`rsshogi-csa`のlocal loopbackで通常対局とponder対局を確認する。
