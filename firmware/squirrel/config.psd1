@{
    VivadoRoot = 'C:\AMDDesignTools\2026.1'

    Pci = @{
        VendorId              = '0x10ee'
        DeviceId              = '0x0666'
        ClassCode             = '0x020000'
        ExpansionRomSizeBytes = 4096
    }

    Upstream = @{
        Repository   = 'https://github.com/ufrisk/pcileech-fpga.git'
        Commit       = 'c538c4170678c13f723dc921905fb81ff3c71d8e'
        CheckoutPath = 'target\fpga'
    }

    OpenOcd = @{
        ArchiveUri    = 'https://docs.lambdaconcept.com/screamer/_downloads/e72a9b76299cd3a4cb30e53dd62505ff/openocd-win.zip'
        ArchiveSha256 = '9d04679ea39bd3ff710f6cfb10e5e4a60f5c166971e0a39c136ae9fc96bf6577'

        FlashSupportUri    = 'https://docs.lambdaconcept.com/screamer/_downloads/20c4c1c1dc18e10efea198d236ac015f/flash_screamer.zip'
        FlashSupportSha256 = '6df1dee88df0805017146b29b9d14cbd77828d4407821be20a3e42d5f86eb9f1'
    }
}
