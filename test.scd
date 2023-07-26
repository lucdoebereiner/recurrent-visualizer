Server.default.options.blockSize = 1
Server.default.options.numOutputBusChannels = 3
Server.default.reboot


{ [SinOsc.ar(132), SinOsc.ar(1321), SinOsc.ar(993)] }.play


{ [SinOsc.ar(7), SinOsc.ar(7), SinOsc.ar(17)] }.play


{ SinOsc.ar(LFNoise1.ar(100!3).exprange(10,1000)) }.play


{ [LFNoise0.ar(3), LFNoise0.ar(2), LFNoise0.ar(3)] }.play


{ [LFNoise1.ar(3), LFNoise1.ar(2), LFNoise1.ar(3)] }.play


{ [LFNoise0.ar(1300), LFNoise0.ar(1200), LFNoise0.ar(1300)] }.play


{ LPF.ar([LFNoise0.ar(131), LFNoise0.ar(321), LFNoise0.ar(193)], 5) }.play


{ LPF.ar([LFNoise1.ar(131), LFNoise1.ar(321), LFNoise1.ar(193)], 1) }.play



(
~source1 = Buffer.readChannel(s,"/home/luc/Dropbox/music/trio23/02-david-230406_180449.wav", startFrame: (48000*60*5), numFrames: (48000*60*5), channels:[0]);

~source2 = Buffer.readChannel(s,"/home/luc/Dropbox/music/trio23/04-ji-230406_180449.wav", startFrame: (48000*60*5), numFrames: (48000*60*5), channels:[0]);

~source3 = Buffer.readChannel(s,"/home/luc/Dropbox/music/trio23/03-leonie-230406_180449.wav", startFrame: (48000*60*5), numFrames: (48000*60*5), channels:[0]);
)

{ [PlayBuf.ar(1, ~source1.bufnum, loop: 1), PlayBuf.ar(1, ~source2.bufnum, loop: 1), PlayBuf.ar(1, ~source3.bufnum, loop: 1)] }.play



{ LPF.ar([PlayBuf.ar(1, ~source1.bufnum, loop: 1), PlayBuf.ar(1, ~source2.bufnum, loop: 1)*4, PlayBuf.ar(1, ~source3.bufnum, loop: 1)], 1) }.play