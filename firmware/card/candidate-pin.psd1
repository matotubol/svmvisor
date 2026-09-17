@{
    # Candidate produced by the OLD Python native-resident pipeline (pre `cargo xtask`).
    # Every value below is specific to that one built candidate and its evidence.
    # RE-BASELINE after the next `cargo xtask resident` + build-card run: recompute
    # Candidate/ResidentBuild/KnownWorking and every Inputs hash and byte count from
    # the freshly built candidate and its evidence directories. flash-card.ps1 loads
    # and strictly validates this file; hardware/tool pins stay in flash-card.ps1.
    Candidate     = 'target/firmware/card/native-resident/ec762c3d176248e4bda8a2532b1e0f53'
    ResidentBuild = 'work/native-firstboot-2026-09-14/build-ack-production'
    KnownWorking  = '4d3d738898a4e49b098a4ae7f940894c7a7c9c4a38e1607345c3551ce1e3efc7'
    Inputs = @(
        @{ Name = 'combined.bin';                  Path = 'target/firmware/card/native-resident/ec762c3d176248e4bda8a2532b1e0f53/combined/combined-review.bin';    Bytes = 5242880; Hash = '8c4f30fcff04cd2f4b2852048ffab3204287ba7a8e31ac019c40528046f53698' }
        @{ Name = 'configuration.bin';             Path = 'target/firmware/card/native-resident/ec762c3d176248e4bda8a2532b1e0f53/svmvisor-endpoint.bin';           Bytes = 1155328; Hash = 'f6311b9ad55428d06932515d0a2b9f2ca02130a6394f3588fc7d49c4990f8fe9' }
        @{ Name = 'payload-slot.bin';              Path = 'target/firmware/card/native-resident/ec762c3d176248e4bda8a2532b1e0f53/combined/payload-slot.bin';       Bytes = 1048576; Hash = 'a932d55e30d35fb970029b29a4166396047fe21f92591c4e0ac448b1deadc728' }
        @{ Name = 'pe-header.bin';                 Path = 'target/firmware/card/native-resident/ec762c3d176248e4bda8a2532b1e0f53/combined/pe-header.bin';          Bytes = 128;     Hash = 'b04b98f711fda533ad1723528c37e3cd742a6869b203e876036b61cc82c539b5' }
        @{ Name = 'native-child.efi';              Path = 'target/firmware/card/native-resident/ec762c3d176248e4bda8a2532b1e0f53/reviewed-child.efi';              Bytes = 153088;  Hash = 'f05bc4e826b31d22723e85103b0afa2b80c310666b6ba73a662503a2a73aee44' }
        @{ Name = 'candidate-manifest.json';       Path = 'target/firmware/card/native-resident/ec762c3d176248e4bda8a2532b1e0f53/manifest.json';                   Bytes = 45329;   Hash = '398b2fec8d1547aa29eb3db7e9e3eb78a7cbfa02ca0958d460dcc89cf33c4e79' }
        @{ Name = 'payload-manifest.json';         Path = 'target/firmware/card/native-resident/ec762c3d176248e4bda8a2532b1e0f53/combined/payload-manifest.json';  Bytes = 1307;    Hash = 'e4f240b8660ec0a06e626e1dee8f9ca580b181d573f4aac4717541633e506392' }
        @{ Name = 'local-review.json';             Path = 'work/native-firstboot-2026-09-14/resident-ack-candidate-review.json';                                   Bytes = 40757;   Hash = '32db46ab5c2bce3399cb7554f8fdf1a0cbc4c1ac40869c6f4a298ab844c6e5c0' }
        @{ Name = 'resident-summary.json';         Path = 'work/native-firstboot-2026-09-14/build-ack-production/summary.json';                                     Bytes = 2523;    Hash = '83d561567b17d0c500540ca381e7c0b1389e29c22ac273527de25c89aea271da' }
        @{ Name = 'resident-source-manifest.json'; Path = 'work/native-firstboot-2026-09-14/build-ack-production/source-manifest.json';                             Bytes = 28783;   Hash = 'ca2f89694a05a1c6264750f4cf272ad36a540a68827cbe4544c4dd8c7109c054' }
    )
}
